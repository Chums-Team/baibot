use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use mxlink::helpers::encryption::EncryptionKey;
use mxlink::matrix_sdk::ruma::{OwnedDeviceId, OwnedUserId};
use serde::{Deserialize, Deserializer, Serialize};
use zeroize::Zeroizing;

use crate::{
    agent::{AgentDefinition, AgentPurpose, PublicIdentifier},
    entity::{globalconfig::GlobalConfig, roomconfig::RoomSettingsHandler},
};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub homeserver: ConfigHomeserver,

    pub user: ConfigUser,

    pub persistence: PersistenceConfig,

    #[serde(default = "super::defaults::command_prefix")]
    pub command_prefix: String,

    #[serde(default)]
    pub room: ConfigRoom,

    /// Localization of the bot's replies. Optional: defaults apply without the section.
    #[serde(default)]
    pub i18n: super::i18n::ConfigI18n,

    pub access: ConfigAccess,

    pub agents: ConfigAgents,

    // Contains the initial global configuration values.
    // Not all properties of the object make sense to be configured statically,
    // so not all of them will be reflected onto the actual global configuration.
    pub initial_global_config: ConfigInitialGlobalConfig,

    #[serde(default = "super::defaults::logging")]
    pub logging: String,

    /// Optional. When absent, the bot runs without billing.
    #[serde(default)]
    pub billing: Option<super::billing::ConfigBilling>,

    /// Optional. Top-ups of the billing ledger through the x402 payment sidecar.
    /// Requires `billing`.
    #[serde(default)]
    pub x402: Option<super::x402::ConfigX402>,
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.homeserver.validate()?;
        self.user.validate(&self.homeserver.server_name)?;
        self.persistence.validate()?;
        self.i18n.validate()?;
        self.room.validate(&self.i18n.fallback_locale)?;
        self.access.validate()?;

        if self.command_prefix.is_empty() {
            return Err(anyhow::anyhow!(
                "The command_prefix ({}) configuration must be set",
                super::env::BAIBOT_COMMAND_PREFIX
            ));
        }

        self.agents.validate()?;
        self.initial_global_config.clone().validate()?;

        if let Some(billing) = &self.billing {
            billing.validate()?;
        }

        if let Some(x402) = &self.x402 {
            if self.billing.is_none() {
                return Err(anyhow::anyhow!(
                    "The x402 configuration section requires the billing section: top-ups are credited to the billing ledger"
                ));
            }

            x402.validate()?;
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum ConfigUserAuth {
    UserPassword {
        username: String,
        password: String,
    },
    AccessToken {
        user_id: OwnedUserId,
        device_id: OwnedDeviceId,
        access_token: String,
    },
    /// Login through a TRON wallet (`user.tron`), see `src/tron_login`.
    Tron {
        username: String,
        user_id: OwnedUserId,
        key: TronKeySource,
        origin: String,
    },
}

/// The wallet key of [`ConfigUserAuth::Tron`], as configured.
///
/// `Debug` does not print the key.
pub enum TronKeySource {
    PrivateKeyHex(String),
    SeedPhrase(String),
}

impl fmt::Debug for TronKeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrivateKeyHex(_) => f.write_str("PrivateKeyHex([redacted])"),
            Self::SeedPhrase(_) => f.write_str("SeedPhrase([redacted])"),
        }
    }
}

#[cfg(feature = "tron-login")]
impl TronKeySource {
    /// The signer for the key, or the reason the key is unusable.
    pub fn signer(&self) -> Result<crate::tron_login::TronSigner, crate::tron_login::TronKeyError> {
        use crate::tron_login::TronSigner;

        match self {
            Self::PrivateKeyHex(private_key) => TronSigner::from_private_key_hex(private_key),
            Self::SeedPhrase(seed_phrase) => TronSigner::from_seed_phrase(seed_phrase),
        }
    }
}

/// TRON wallet authentication (`user.tron`), see docs/configuration/authentication.md.
///
/// The homeserver logs the bot in as the Matrix user the wallet is bound to.
/// `Debug` does not print the key material.
#[derive(Default, Serialize, Deserialize)]
pub struct ConfigUserTron {
    /// The wallet's private key: 32 bytes in hex. Either this or `seed_phrase`.
    #[serde(default)]
    pub private_key: Option<String>,

    /// The wallet's BIP-39 seed phrase (12 to 24 English words). Either this or `private_key`.
    #[serde(default)]
    pub seed_phrase: Option<String>,

    /// The `origin` reported to the homeserver when asking for a login challenge; it must be
    /// listed in the homeserver's `tron_auth_allowed_origins`. Defaults to the value the Chums
    /// homeserver lists for this bot.
    #[serde(default)]
    pub origin: Option<String>,
}

impl fmt::Debug for ConfigUserTron {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigUserTron")
            .field(
                "private_key",
                &self.private_key.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "seed_phrase",
                &self.seed_phrase.as_ref().map(|_| "[redacted]"),
            )
            .field("origin", &self.origin)
            .finish()
    }
}

impl ConfigUserTron {
    fn key_source(&self) -> anyhow::Result<TronKeySource> {
        let private_key = self
            .private_key
            .as_deref()
            .filter(|value| !value.is_empty());
        let seed_phrase = self
            .seed_phrase
            .as_deref()
            .filter(|value| !value.is_empty());

        match (private_key, seed_phrase) {
            (Some(private_key), None) => Ok(TronKeySource::PrivateKeyHex(private_key.to_owned())),
            (None, Some(seed_phrase)) => Ok(TronKeySource::SeedPhrase(seed_phrase.to_owned())),
            (Some(_), Some(_)) => Err(anyhow::anyhow!(
                "Set exactly one of user.tron.private_key ({}) and user.tron.seed_phrase ({})",
                super::env::BAIBOT_USER_TRON_PRIVATE_KEY,
                super::env::BAIBOT_USER_TRON_SEED_PHRASE
            )),
            (None, None) => Err(anyhow::anyhow!(
                "The user.tron section needs the wallet key: set user.tron.private_key ({}) or user.tron.seed_phrase ({})",
                super::env::BAIBOT_USER_TRON_PRIVATE_KEY,
                super::env::BAIBOT_USER_TRON_SEED_PHRASE
            )),
        }
    }

    fn origin(&self) -> String {
        self.origin
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(super::defaults::user_tron_origin)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigHomeserver {
    pub server_name: String,
    pub url: String,
}

impl ConfigHomeserver {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.server_name.is_empty() {
            return Err(anyhow::anyhow!(
                "The homeserver.server_name ({}) configuration must be set",
                super::env::BAIBOT_HOMESERVER_SERVER_NAME
            ));
        }

        if self.url.is_empty() {
            return Err(anyhow::anyhow!(
                "The homeserver.url ({}) configuration must be set",
                super::env::BAIBOT_HOMESERVER_URL
            ));
        }

        Ok(())
    }
}

/// Configuration for the bot's avatar.
///
/// - `Default`: Use the built-in default avatar (null, empty string, or missing in config)
/// - `Keep`: Don't touch the avatar, keep whatever is already set ("keep" in config)
/// - `Custom(String)`: Use a custom avatar from the specified file path
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub enum Avatar {
    /// Use the built-in default avatar
    #[default]
    Default,
    /// Keep the current avatar, don't change it
    Keep,
    /// Use a custom avatar from the specified file path
    Custom(String),
}

impl<'de> Deserialize<'de> for Avatar {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value: Option<String> = Option::deserialize(deserializer)?;
        Ok(match value {
            None => Avatar::Default,
            Some(s) => Avatar::from_string(s),
        })
    }
}

impl Avatar {
    pub fn from_string(value: String) -> Self {
        if value.is_empty() {
            Avatar::Default
        } else if value.eq_ignore_ascii_case("keep") {
            Avatar::Keep
        } else {
            Avatar::Custom(value)
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigUser {
    pub mxid_localpart: String,

    #[serde(default)]
    pub password: Option<String>,

    #[serde(default)]
    pub access_token: Option<String>,

    #[serde(default)]
    pub device_id: Option<String>,

    /// Optional. Login through a TRON wallet instead of a password or an access token.
    #[serde(default)]
    pub tron: Option<ConfigUserTron>,

    #[serde(default = "super::defaults::name")]
    pub name: String,

    #[serde(default)]
    pub encryption: ConfigUserEncryption,

    #[serde(default)]
    pub avatar: Avatar,
}

impl ConfigUser {
    pub fn validate(&self, homeserver_server_name: &str) -> anyhow::Result<()> {
        if self.mxid_localpart.is_empty() {
            return Err(anyhow::anyhow!(
                "The user.mxid_localpart ({}) configuration must be set",
                super::env::BAIBOT_USER_MXID_LOCALPART
            ));
        }

        self.auth_config(homeserver_server_name)?;

        if self.name.is_empty() {
            return Err(anyhow::anyhow!(
                "The name ({}) configuration must be set",
                super::env::BAIBOT_USER_NAME
            ));
        }

        self.encryption.validate()?;

        Ok(())
    }

    pub fn auth_config(&self, homeserver_server_name: &str) -> anyhow::Result<ConfigUserAuth> {
        let password = self.password.as_deref().filter(|value| !value.is_empty());
        let access_token = self
            .access_token
            .as_deref()
            .filter(|value| !value.is_empty());
        let tron = self.tron.as_ref();

        let modes_set = [password.is_some(), access_token.is_some(), tron.is_some()]
            .into_iter()
            .filter(|set| *set)
            .count();

        if modes_set > 1 {
            return Err(anyhow::anyhow!(
                "Set exactly one authentication method: user.password ({}), user.access_token ({}) + user.device_id ({}), or user.tron ({}, {})",
                super::env::BAIBOT_USER_PASSWORD,
                super::env::BAIBOT_USER_ACCESS_TOKEN,
                super::env::BAIBOT_USER_DEVICE_ID,
                super::env::BAIBOT_USER_TRON_PRIVATE_KEY,
                super::env::BAIBOT_USER_TRON_SEED_PHRASE
            ));
        }

        if let Some(password) = password {
            return Ok(ConfigUserAuth::UserPassword {
                username: self.mxid_localpart.to_owned(),
                password: password.to_owned(),
            });
        }

        if let Some(access_token) = access_token {
            let device_id = self
                .device_id
                .as_deref()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "user.device_id ({}) must be set when using access token authentication",
                        super::env::BAIBOT_USER_DEVICE_ID
                    )
                })?;

            return Ok(ConfigUserAuth::AccessToken {
                user_id: self.user_id(homeserver_server_name)?,
                device_id: OwnedDeviceId::from(device_id),
                access_token: access_token.to_owned(),
            });
        }

        if let Some(tron) = tron {
            let key = tron.key_source()?;
            let origin = tron.origin();

            #[cfg(not(feature = "tron-login"))]
            {
                let _ = (key, origin);
                return Err(anyhow::anyhow!(
                    "The user.tron section is set, but this build of the bot has no TRON wallet login: it was built without the `tron-login` cargo feature"
                ));
            }

            #[cfg(feature = "tron-login")]
            {
                // Fail at start-up rather than at login time on an unusable key.
                key.signer()
                    .map_err(|err| anyhow::anyhow!("user.tron: {err}"))?;

                return Ok(ConfigUserAuth::Tron {
                    username: self.mxid_localpart.to_owned(),
                    user_id: self.user_id(homeserver_server_name)?,
                    key,
                    origin,
                });
            }
        }

        Err(anyhow::anyhow!(
            "Set one authentication method: user.password ({}), user.access_token ({}) + user.device_id ({}), or user.tron ({}, {})",
            super::env::BAIBOT_USER_PASSWORD,
            super::env::BAIBOT_USER_ACCESS_TOKEN,
            super::env::BAIBOT_USER_DEVICE_ID,
            super::env::BAIBOT_USER_TRON_PRIVATE_KEY,
            super::env::BAIBOT_USER_TRON_SEED_PHRASE
        ))
    }

    /// The passphrase of the account's secret storage, see `src/recovery.rs`.
    ///
    /// A configured `user.encryption.recovery_passphrase` wins. Without one, the TRON wallet
    /// login derives the passphrase from the wallet key, the same way the Chums web client does
    /// through TronLink, so the bot needs no passphrase of its own. `None` otherwise.
    pub fn recovery_passphrase(
        &self,
        auth: &ConfigUserAuth,
    ) -> anyhow::Result<Option<RecoveryPassphrase>> {
        if let Some(passphrase) = self.encryption.recovery_passphrase.as_deref() {
            return Ok(Some(RecoveryPassphrase {
                value: Zeroizing::new(passphrase.to_owned()),
                source: RecoveryPassphraseSource::Configuration,
            }));
        }

        #[cfg(feature = "tron-login")]
        if let ConfigUserAuth::Tron { key, user_id, .. } = auth {
            let signer = key
                .signer()
                .map_err(|err| anyhow::anyhow!("user.tron: {err}"))?;
            let value = signer.secret_storage_passphrase(user_id).map_err(|err| {
                anyhow::anyhow!(
                    "Deriving the recovery passphrase from the wallet key failed: {err}"
                )
            })?;

            return Ok(Some(RecoveryPassphrase {
                value,
                source: RecoveryPassphraseSource::TronWallet,
            }));
        }

        #[cfg(not(feature = "tron-login"))]
        let _ = auth;

        Ok(None)
    }

    fn user_id(&self, homeserver_server_name: &str) -> anyhow::Result<OwnedUserId> {
        OwnedUserId::try_from(format!(
            "@{}:{}",
            self.mxid_localpart, homeserver_server_name
        ))
        .map_err(|e| anyhow::anyhow!("Invalid user ID: {e}"))
    }
}

/// The secret storage passphrase of [`ConfigUser::recovery_passphrase`]. `Debug` prints the
/// source only.
pub struct RecoveryPassphrase {
    pub value: Zeroizing<String>,
    pub source: RecoveryPassphraseSource,
}

impl fmt::Debug for RecoveryPassphrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryPassphrase")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPassphraseSource {
    /// `user.encryption.recovery_passphrase`.
    Configuration,
    /// Derived from the key of the TRON wallet the bot logs in with.
    TronWallet,
}

impl fmt::Display for RecoveryPassphraseSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Configuration => "the configuration",
            Self::TronWallet => "the TRON wallet key",
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ConfigUserEncryption {
    pub recovery_passphrase: Option<String>,
    pub recovery_reset_allowed: bool,
}

impl ConfigUserEncryption {
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Some(passphrase) = &self.recovery_passphrase
            && passphrase.is_empty()
        {
            return Err(anyhow::anyhow!(
                "The user.encryption.recovery_passphrase ({}) configuration must either be null or set to a non-empty passphrase",
                super::env::BAIBOT_USER_ENCRYPTION_RECOVERY_PASSPHRASE
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PersistenceConfig {
    #[serde(default = "super::defaults::persistence_data_dir_path")]
    pub data_dir_path: Option<String>,

    #[serde(default = "super::defaults::persistence_session_file_name")]
    session_file_name: String,

    #[serde(default = "super::defaults::persistence_db_dir_name")]
    db_dir_name: String,

    pub session_encryption_key: Option<String>,

    pub config_encryption_key: Option<String>,
}

impl PersistenceConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Some(data_dir_path) = &self.data_dir_path {
            let path = PathBuf::from(data_dir_path);
            if !path.exists() {
                return Err(anyhow::anyhow!(
                    "The persistence.data_dir_path ({}) directory ({}) must exist",
                    super::env::BAIBOT_PERSISTENCE_DATA_DIR_PATH,
                    data_dir_path,
                ));
            }
        }

        self.config_encryption_key()
            .map_err(|e| anyhow::anyhow!(e))?;

        Ok(())
    }

    pub fn data_dir_path_or_err(&self) -> anyhow::Result<PathBuf> {
        let Some(data_dir_path) = &self.data_dir_path else {
            return Err(anyhow::anyhow!(
                "The persistence.data_dir_path ({}) directory must be set",
                super::env::BAIBOT_PERSISTENCE_DATA_DIR_PATH
            ));
        };

        Ok(PathBuf::from(data_dir_path))
    }

    pub fn session_file_path(&self) -> anyhow::Result<PathBuf> {
        let mut path = self.data_dir_path_or_err()?;
        path.push(&self.session_file_name);

        Ok(path)
    }

    pub fn db_dir_path(&self) -> anyhow::Result<PathBuf> {
        let mut path = self.data_dir_path_or_err()?;
        path.push(&self.db_dir_name);

        Ok(path)
    }

    pub fn session_encryption_key(&self) -> anyhow::Result<Option<EncryptionKey>> {
        self.parse_encryption_key(&self.session_encryption_key).map_err(|err| {
            anyhow::anyhow!(
                "Encryption key specified in persistence.session_encryption_key ({}) is not valid: {}",
                super::env::BAIBOT_PERSISTENCE_SESSION_ENCRYPTION_KEY,
                err
            )
        })
    }

    pub fn config_encryption_key(&self) -> anyhow::Result<Option<EncryptionKey>> {
        self.parse_encryption_key(&self.config_encryption_key).map_err(|err| {
            anyhow::anyhow!(
                "Encryption key specified in persistence.config_encryption_key ({}) is not valid: {}",
                super::env::BAIBOT_PERSISTENCE_CONFIG_ENCRYPTION_KEY,
                err
            )
        })
    }

    fn parse_encryption_key(
        &self,
        value: &Option<String>,
    ) -> anyhow::Result<Option<EncryptionKey>, String> {
        let key = match value {
            Some(key) => {
                if key.is_empty() {
                    None
                } else {
                    Some(EncryptionKey::from_hex_str(key)?)
                }
            }
            None => None,
        };

        Ok(key)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigRoom {
    #[serde(default = "super::defaults::room_post_join_self_introduction_enabled")]
    pub post_join_self_introduction_enabled: bool,

    /// Optional. A fixed introduction per locale (`en`, `ru`, …) that replaces the built-in
    /// introduction (bot name, agents, commands) after joining a room. The bot sends it in the
    /// locale of the user who invited it, falling back to `i18n.fallback_locale`, whose entry is
    /// therefore required. Empty: the built-in introduction is sent.
    /// See `docs/configuration/i18n.md`.
    #[serde(default)]
    pub post_join_self_introduction_text: BTreeMap<String, String>,
}

impl ConfigRoom {
    pub fn validate(&self, fallback_locale: &str) -> anyhow::Result<()> {
        if self.post_join_self_introduction_text.is_empty() {
            return Ok(());
        }

        let available = crate::i18n::available_locales();

        for (locale, text) in &self.post_join_self_introduction_text {
            if !available.iter().any(|a| a == locale) {
                return Err(anyhow::anyhow!(
                    "The room.post_join_self_introduction_text configuration must be keyed by the locales the bot has translations for ({}), got `{}`",
                    available.join(", "),
                    locale,
                ));
            }

            if text.trim().is_empty() {
                return Err(anyhow::anyhow!(
                    "The room.post_join_self_introduction_text configuration must not have an empty text (locale `{}`)",
                    locale,
                ));
            }
        }

        if !self
            .post_join_self_introduction_text
            .contains_key(fallback_locale)
        {
            return Err(anyhow::anyhow!(
                "The room.post_join_self_introduction_text configuration must have a text for the i18n.fallback_locale (`{}`), the one sent to users without a declared locale",
                fallback_locale,
            ));
        }

        Ok(())
    }
}

impl Default for ConfigRoom {
    fn default() -> Self {
        Self {
            post_join_self_introduction_enabled:
                super::defaults::room_post_join_self_introduction_enabled(),
            post_join_self_introduction_text: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigAccess {
    // Contains the admin whitelist patterns before parsing into regex.
    // Example: `["@*:example.com"]`
    pub admin_patterns: Vec<String>,

    /// Optional. When `true`, the bot's commands (`<command_prefix> help`, `… config`, …) sent
    /// by users who are not administrators are ignored without a reply, except the commands
    /// listed in `commands_admin_exempt`. Conversation (plain text, mentions,
    /// `<command_prefix> <free text>`) is not affected. Default `false`.
    /// See `docs/access.md`.
    #[serde(default)]
    pub commands_admin_only: bool,

    /// Optional. The commands (by their first word: `balance`, `image`, …) that everyone may
    /// use when `commands_admin_only` is on. Default: `balance`, `topup`, `image`.
    #[serde(default = "super::defaults::access_commands_admin_exempt")]
    pub commands_admin_exempt: Vec<String>,
}

impl ConfigAccess {
    /// Whether the command whose first word is `head` is reserved for administrators.
    /// `false` when `commands_admin_only` is off.
    pub fn is_command_admin_only(&self, head: &str) -> bool {
        self.commands_admin_only && !self.commands_admin_exempt.iter().any(|h| h == head)
    }

    // Returns the the mxidwc-parsed regexes for the admin whitelist.
    // Example: `["^@\.*:example\.com$"]`
    pub fn admin_pattern_regexes(&self) -> anyhow::Result<Vec<regex::Regex>> {
        mxidwc::parse_patterns_vector(&self.admin_patterns).map_err(|e| {
            anyhow::anyhow!(
                "Failed parsing access.admin_patterns ({}): {:?}",
                super::env::BAIBOT_ACCESS_ADMIN_PATTERNS,
                e
            )
        })
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.admin_patterns.is_empty() {
            return Err(anyhow::anyhow!(
                "The access.admin_patterns ({}) configuration must contain at least one pattern",
                super::env::BAIBOT_ACCESS_ADMIN_PATTERNS
            ));
        }

        self.admin_pattern_regexes()?;

        let known = crate::controller::COMMAND_HEADS;
        for head in &self.commands_admin_exempt {
            if !known.contains(&head.as_str()) {
                return Err(anyhow::anyhow!(
                    "The access.commands_admin_exempt ({}) configuration contains an unknown command `{}`. Known commands: {}",
                    super::env::BAIBOT_ACCESS_COMMANDS_ADMIN_EXEMPT,
                    head,
                    known.join(", "),
                ));
            }
        }

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConfigAgents {
    pub static_definitions: Vec<AgentDefinition>,
}

impl ConfigAgents {
    pub fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigInitialGlobalConfig {
    #[serde(default)]
    pub handler: RoomSettingsHandler,

    pub user_patterns: Option<Vec<String>>,
}

impl ConfigInitialGlobalConfig {
    fn user_pattern_regexes(&self) -> anyhow::Result<Option<Vec<regex::Regex>>> {
        match &self.user_patterns {
            Some(user_patterns) => {
                let user_patterns = mxidwc::parse_patterns_vector(user_patterns).map_err(|e| {
                    anyhow::anyhow!(
                        "Failed parsing initial_global_config.user_patterns ({}): {}",
                        super::env::BAIBOT_INITIAL_GLOBAL_CONFIG_USER_PATTERNS,
                        e
                    )
                })?;

                Ok(Some(user_patterns))
            }
            None => Ok(None),
        }
    }

    pub fn validate(self) -> anyhow::Result<()> {
        self.user_pattern_regexes()?;

        for purpose in AgentPurpose::choices() {
            let agent_id = self.handler.get_by_purpose(*purpose);

            let Some(agent_id) = agent_id else {
                // None is OK
                continue;
            };

            let config_key = format!(
                "initial_global_config.handler.{}",
                purpose.as_str().replace("-", "_")
            );

            if agent_id.is_empty() {
                return Err(anyhow::anyhow!(
                    "The {} configuration key must be pointing to a valid agent id or be set to null",
                    config_key,
                ));
            }

            let agent_identifier = PublicIdentifier::from_str(&agent_id);

            let Some(agent_identifier) = agent_identifier else {
                return Err(anyhow::anyhow!(
                    "The {} configuration key specifies an agent id (`{}`) that cannot be parsed. {}",
                    config_key,
                    agent_id,
                    crate::strings::agent::invalid_id_generic()
                ));
            };

            // We only allow statically-defined agents for now, although DynamicGlobal may make sense too.
            let PublicIdentifier::Static(_) = agent_identifier else {
                return Err(anyhow::anyhow!(
                    "The {} configuration key specifies an agent id (`{}`) which does not refer to a static agent.",
                    config_key,
                    agent_id,
                ));
            };
        }

        let _: GlobalConfig = self.try_into()?;

        Ok(())
    }
}

impl TryInto<GlobalConfig> for ConfigInitialGlobalConfig {
    type Error = anyhow::Error;

    fn try_into(self) -> anyhow::Result<GlobalConfig> {
        let mut entity = GlobalConfig::default();

        if let Some(user_patterns) = self.user_patterns {
            // We'd rather fail parsing this during startup than at runtime
            let _ = mxidwc::parse_patterns_vector(&user_patterns).map_err(|err| {
                anyhow::anyhow!(
                    "Bad initial_global_config.user_patterns ({}): {}",
                    super::env::BAIBOT_INITIAL_GLOBAL_CONFIG_USER_PATTERNS,
                    err
                )
            })?;

            entity.access.user_patterns = if user_patterns.is_empty() {
                None
            } else {
                Some(user_patterns)
            };
        }

        for purpose in AgentPurpose::choices() {
            let agent_id = self.handler.get_by_purpose(*purpose);

            entity
                .fallback_room_settings
                .handler
                .set_by_purpose(*purpose, agent_id);
        }

        Ok(entity)
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
