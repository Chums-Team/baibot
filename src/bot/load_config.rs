use std::env;
use std::path::PathBuf;

use anyhow::anyhow;

use crate::agent::AgentPurpose;

pub use crate::entity::cfg::{
    Avatar, Config, ConfigBilling, ConfigX402, defaults as cfg_defaults, env as cfg_env,
};

pub fn load() -> anyhow::Result<Config> {
    let config_file_path = env::var(cfg_env::BAIBOT_CONFIG_FILE_PATH)
        .unwrap_or_else(|_| cfg_defaults::config_file_path().to_owned());
    let config_file_path = PathBuf::from(config_file_path);

    if !config_file_path.exists() {
        return Err(anyhow!(
            "Config file ({}) not found. Adjust the {} environment variable to use another config file.",
            config_file_path.display(),
            cfg_env::BAIBOT_CONFIG_FILE_PATH,
        ));
    }

    let config_str = std::fs::read_to_string(config_file_path)?;
    let mut config: Config = serde_yaml_ng::from_str(&config_str)?;

    // Allow environment variables to override some configuration keys
    for (key, value) in env::vars() {
        match key.as_str() {
            cfg_env::BAIBOT_HOMESERVER_SERVER_NAME => config.homeserver.server_name = value,
            cfg_env::BAIBOT_HOMESERVER_URL => config.homeserver.url = value,
            cfg_env::BAIBOT_USER_MXID_LOCALPART => config.user.mxid_localpart = value,
            cfg_env::BAIBOT_USER_PASSWORD => {
                config.user.password = optional_non_empty(value);
            }
            cfg_env::BAIBOT_USER_ACCESS_TOKEN => {
                config.user.access_token = optional_non_empty(value);
            }
            cfg_env::BAIBOT_USER_DEVICE_ID => {
                config.user.device_id = optional_non_empty(value);
            }
            cfg_env::BAIBOT_USER_ENCRYPTION_RECOVERY_PASSPHRASE => {
                config.user.encryption.recovery_passphrase = Some(value);
            }
            cfg_env::BAIBOT_USER_ENCRYPTION_RECOVERY_RESET_ALLOWED => {
                config.user.encryption.recovery_reset_allowed = value.parse::<bool>()?;
            }
            cfg_env::BAIBOT_USER_NAME => config.user.name = value,
            cfg_env::BAIBOT_USER_AVATAR => {
                config.user.avatar = Avatar::from_string(value);
            }
            cfg_env::BAIBOT_COMMAND_PREFIX => config.command_prefix = value,
            cfg_env::BAIBOT_ROOM_POST_JOIN_SELF_INTRODUCTION_ENABLED => {
                config.room.post_join_self_introduction_enabled = value.parse::<bool>()?;
            }
            cfg_env::BAIBOT_I18N_FALLBACK_LOCALE => {
                config.i18n.fallback_locale = value;
            }
            cfg_env::BAIBOT_LOGGING => {
                config.logging = value;
            }
            cfg_env::BAIBOT_ACCESS_ADMIN_PATTERNS => {
                config.access.admin_patterns = value
                    .split(' ')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            cfg_env::BAIBOT_ACCESS_COMMANDS_ADMIN_ONLY => {
                config.access.commands_admin_only = value.parse::<bool>()?;
            }
            cfg_env::BAIBOT_ACCESS_COMMANDS_ADMIN_EXEMPT => {
                config.access.commands_admin_exempt = value
                    .split([' ', ','])
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            cfg_env::BAIBOT_PERSISTENCE_DATA_DIR_PATH => {
                config.persistence.data_dir_path = Some(value);
            }
            cfg_env::BAIBOT_PERSISTENCE_SESSION_ENCRYPTION_KEY => {
                config.persistence.session_encryption_key = Some(value);
            }
            cfg_env::BAIBOT_PERSISTENCE_CONFIG_ENCRYPTION_KEY => {
                config.persistence.config_encryption_key = Some(value);
            }
            cfg_env::BAIBOT_INITIAL_GLOBAL_CONFIG_HANDLER_CATCH_ALL => {
                let value = if value.is_empty() { None } else { Some(value) };

                config
                    .initial_global_config
                    .handler
                    .set_by_purpose(AgentPurpose::CatchAll, value);
            }
            cfg_env::BAIBOT_INITIAL_GLOBAL_CONFIG_HANDLER_TEXT_GENERATION => {
                let value = if value.is_empty() { None } else { Some(value) };

                config
                    .initial_global_config
                    .handler
                    .set_by_purpose(AgentPurpose::TextGeneration, value);
            }
            cfg_env::BAIBOT_INITIAL_GLOBAL_CONFIG_HANDLER_TEXT_TO_SPEECH => {
                let value = if value.is_empty() { None } else { Some(value) };

                config
                    .initial_global_config
                    .handler
                    .set_by_purpose(AgentPurpose::TextToSpeech, value);
            }
            cfg_env::BAIBOT_INITIAL_GLOBAL_CONFIG_HANDLER_SPEECH_TO_TEXT => {
                let value = if value.is_empty() { None } else { Some(value) };

                config
                    .initial_global_config
                    .handler
                    .set_by_purpose(AgentPurpose::SpeechToText, value);
            }
            cfg_env::BAIBOT_INITIAL_GLOBAL_CONFIG_HANDLER_IMAGE_GENERATION => {
                let value = if value.is_empty() { None } else { Some(value) };

                config
                    .initial_global_config
                    .handler
                    .set_by_purpose(AgentPurpose::ImageGeneration, value);
            }
            cfg_env::BAIBOT_INITIAL_GLOBAL_CONFIG_USER_PATTERNS => {
                config.initial_global_config.user_patterns = Some(
                    value
                        .split(' ')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                );
            }
            cfg_env::BAIBOT_BILLING_RESERVE_AMOUNT_USD => {
                billing_section(&mut config).reserve_amount_usd = parse_f64(&key, &value)?;
            }
            cfg_env::BAIBOT_BILLING_MARKUP_PCT => {
                billing_section(&mut config).markup_pct = parse_f64(&key, &value)?;
            }
            cfg_env::BAIBOT_BILLING_DAILY_CAP_USD => {
                billing_section(&mut config).daily_cap_usd = parse_f64(&key, &value)?;
            }
            cfg_env::BAIBOT_BILLING_MONTHLY_CAP_USD => {
                billing_section(&mut config).monthly_cap_usd = parse_f64(&key, &value)?;
            }
            cfg_env::BAIBOT_BILLING_MIN_TOPUP_USD => {
                billing_section(&mut config).min_topup_usd = parse_f64(&key, &value)?;
            }
            cfg_env::BAIBOT_BILLING_MAX_TOPUP_USD => {
                billing_section(&mut config).max_topup_usd = parse_f64(&key, &value)?;
            }
            cfg_env::BAIBOT_BILLING_ADMIN_MXIDS => {
                billing_section(&mut config).admin_mxids = value
                    .split([' ', ','])
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            cfg_env::BAIBOT_BILLING_DB_PATH => {
                billing_section(&mut config).db_path = optional_non_empty(value);
            }
            cfg_env::BAIBOT_X402_SIDECAR_URL => {
                x402_section(&mut config).sidecar_url = value;
            }
            cfg_env::BAIBOT_X402_INTERNAL_SECRET => {
                x402_section(&mut config).internal_secret = value;
            }
            cfg_env::BAIBOT_X402_INTERNAL_BIND => {
                x402_section(&mut config).internal_bind = value.parse().map_err(|e| {
                    anyhow!("The {key} environment variable must be an IP address: {e}")
                })?;
            }
            cfg_env::BAIBOT_X402_INTERNAL_PORT => {
                x402_section(&mut config).internal_port = value.parse().map_err(|e| {
                    anyhow!("The {key} environment variable must be a port number: {e}")
                })?;
            }
            cfg_env::BAIBOT_X402_ALLOW_NON_LOOPBACK_BIND => {
                x402_section(&mut config).allow_non_loopback_bind = value.parse().map_err(|e| {
                    anyhow!("The {key} environment variable must be true or false: {e}")
                })?;
            }
            _ => {
                if let Some(locale) =
                    key.strip_prefix(cfg_env::BAIBOT_ROOM_POST_JOIN_SELF_INTRODUCTION_TEXT_PREFIX)
                {
                    set_post_join_self_introduction_text(&mut config, locale, value);
                }
            }
        }
    }

    config.validate().map_err(|s| anyhow!(s))?;

    Ok(config)
}

fn optional_non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

/// `BAIBOT_ROOM_POST_JOIN_SELF_INTRODUCTION_TEXT_<LOCALE>`: the locale comes from the variable
/// name (`EN` → `en`); an empty value removes the text the configuration file has for it.
fn set_post_join_self_introduction_text(config: &mut Config, locale: &str, value: String) {
    let locale = locale.to_ascii_lowercase();
    let texts = &mut config.room.post_join_self_introduction_text;

    match optional_non_empty(value) {
        Some(text) => {
            texts.insert(locale, text);
        }
        None => {
            texts.remove(&locale);
        }
    }
}

/// Setting any `BAIBOT_BILLING_*` variable enables billing even when the configuration
/// file has no `billing` section, so a deployment can turn it on from the environment alone.
fn billing_section(config: &mut Config) -> &mut ConfigBilling {
    config.billing.get_or_insert_with(ConfigBilling::default)
}

/// Same rule for `BAIBOT_X402_*`: any of them creates the `x402` section.
fn x402_section(config: &mut Config) -> &mut ConfigX402 {
    config.x402.get_or_insert_with(ConfigX402::default)
}

fn parse_f64(key: &str, value: &str) -> anyhow::Result<f64> {
    value
        .parse::<f64>()
        .map_err(|e| anyhow!("The {key} environment variable must be a number: {e}"))
}
