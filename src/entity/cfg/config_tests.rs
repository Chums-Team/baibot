#[cfg(feature = "tron-login")]
use super::TronKeySource;
use super::{Avatar, ConfigUser, ConfigUserAuth, ConfigUserEncryption, ConfigUserTron};
use crate::entity::cfg::env;

fn base_user() -> ConfigUser {
    ConfigUser {
        mxid_localpart: "baibot".to_owned(),
        password: None,
        access_token: None,
        device_id: None,
        tron: None,
        name: "baibot".to_owned(),
        encryption: ConfigUserEncryption {
            recovery_passphrase: None,
            recovery_reset_allowed: false,
        },
        avatar: Avatar::Default,
    }
}

#[test]
fn auth_config_uses_password_mode() {
    let mut user = base_user();
    user.password = Some("secret".to_owned());

    let auth = user
        .auth_config("example.com")
        .expect("password auth should be valid");

    match auth {
        ConfigUserAuth::UserPassword { username, password } => {
            assert_eq!(username, "baibot");
            assert_eq!(password, "secret");
        }
        other => panic!("expected password auth mode, got {other:?}"),
    }
}

#[test]
fn auth_config_uses_access_token_mode() {
    let mut user = base_user();
    user.access_token = Some("token123".to_owned());
    user.device_id = Some("DEVICE1".to_owned());

    let auth = user
        .auth_config("example.com")
        .expect("access token auth should be valid");

    match auth {
        ConfigUserAuth::AccessToken {
            user_id,
            device_id,
            access_token,
        } => {
            assert_eq!(user_id.as_str(), "@baibot:example.com");
            assert_eq!(device_id.as_str(), "DEVICE1");
            assert_eq!(access_token, "token123");
        }
        other => panic!("expected access token auth mode, got {other:?}"),
    }
}

#[test]
fn auth_config_rejects_both_auth_methods() {
    let mut user = base_user();
    user.password = Some("secret".to_owned());
    user.access_token = Some("token123".to_owned());
    user.device_id = Some("DEVICE1".to_owned());

    let err = user
        .auth_config("example.com")
        .expect_err("both auth methods should be rejected");

    assert!(
        err.to_string()
            .contains("exactly one authentication method")
    );
}

#[test]
fn auth_config_rejects_missing_auth() {
    let user = base_user();

    let err = user
        .auth_config("example.com")
        .expect_err("missing auth should be rejected");

    assert!(err.to_string().contains("Set one authentication method"));
}

#[test]
fn auth_config_rejects_access_token_without_device_id() {
    let mut user = base_user();
    user.access_token = Some("token123".to_owned());

    let err = user
        .auth_config("example.com")
        .expect_err("access token mode without device_id should be rejected");

    assert!(err.to_string().contains(env::BAIBOT_USER_DEVICE_ID));
}

#[test]
fn auth_config_treats_empty_strings_as_unset() {
    let mut user = base_user();
    user.password = Some(String::new());
    user.access_token = Some(String::new());
    user.device_id = Some(String::new());

    let err = user
        .auth_config("example.com")
        .expect_err("empty auth values should be treated as unset");

    assert!(err.to_string().contains("Set one authentication method"));
}

mod room {
    use std::collections::BTreeMap;

    use crate::entity::cfg::config::ConfigRoom;

    fn room(texts: &[(&str, &str)]) -> ConfigRoom {
        ConfigRoom {
            post_join_self_introduction_text: texts
                .iter()
                .map(|(locale, text)| ((*locale).to_owned(), (*text).to_owned()))
                .collect(),
            ..ConfigRoom::default()
        }
    }

    #[test]
    fn default_has_no_introduction_text() {
        let cfg = ConfigRoom::default();
        assert!(cfg.post_join_self_introduction_enabled);
        assert_eq!(cfg.post_join_self_introduction_text, BTreeMap::new());
        cfg.validate("en").unwrap();
    }

    #[test]
    fn parses_the_text_map() {
        let cfg: ConfigRoom = serde_yaml_ng::from_str(
            "post_join_self_introduction_text:\n  en: \"Hi!\"\n  ru: \"Привет!\"\n",
        )
        .unwrap();
        assert_eq!(cfg.post_join_self_introduction_text["ru"], "Привет!");
        cfg.validate("en").unwrap();
    }

    #[test]
    fn requires_a_text_for_the_fallback_locale() {
        let err = room(&[("ru", "Привет!")])
            .validate("en")
            .unwrap_err()
            .to_string();
        assert!(err.contains("i18n.fallback_locale"), "{err}");
        assert!(err.contains("`en`"), "{err}");

        room(&[("ru", "Привет!")]).validate("ru").unwrap();
    }

    #[test]
    fn rejects_a_locale_without_translations() {
        let err = room(&[("en", "Hi!"), ("xx", "?")])
            .validate("en")
            .unwrap_err()
            .to_string();
        assert!(err.contains("`xx`"), "{err}");
        assert!(err.contains("en, "), "{err}");

        // Exact file names, as for i18n.fallback_locale.
        assert!(
            room(&[("en", "Hi!"), ("EN", "Hi!")])
                .validate("en")
                .is_err()
        );
        assert!(
            room(&[("en", "Hi!"), ("pt-BR", "Olá!")])
                .validate("en")
                .is_err()
        );
    }

    #[test]
    fn rejects_an_empty_text() {
        let err = room(&[("en", "Hi!"), ("ru", "  ")])
            .validate("en")
            .unwrap_err()
            .to_string();
        assert!(err.contains("empty text"), "{err}");
        assert!(err.contains("`ru`"), "{err}");
    }
}

mod access {
    use super::super::ConfigAccess;

    fn access(commands_admin_only: bool, exempt: &[&str]) -> ConfigAccess {
        ConfigAccess {
            admin_patterns: vec!["@admin:example.com".to_owned()],
            commands_admin_only,
            commands_admin_exempt: exempt.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn defaults_keep_the_commands_open() {
        let cfg: ConfigAccess =
            serde_yaml_ng::from_str("admin_patterns: ['@admin:example.com']").unwrap();

        assert!(!cfg.commands_admin_only);
        assert_eq!(cfg.commands_admin_exempt, ["balance", "topup", "image"]);
        assert!(!cfg.is_command_admin_only("help"));
        cfg.validate().unwrap();
    }

    #[test]
    fn parses_the_gate() {
        let cfg: ConfigAccess = serde_yaml_ng::from_str(
            "admin_patterns: ['@admin:example.com']\ncommands_admin_only: true\ncommands_admin_exempt: [balance]",
        )
        .unwrap();

        assert!(cfg.commands_admin_only);
        assert_eq!(cfg.commands_admin_exempt, ["balance"]);
        cfg.validate().unwrap();
    }

    #[test]
    fn exempt_commands_are_open_when_the_gate_is_on() {
        let cfg = access(true, &["balance", "topup"]);

        assert!(cfg.is_command_admin_only("help"));
        assert!(cfg.is_command_admin_only("config"));
        assert!(!cfg.is_command_admin_only("balance"));
        assert!(!cfg.is_command_admin_only("topup"));
    }

    #[test]
    fn nothing_is_admin_only_when_the_gate_is_off() {
        let cfg = access(false, &[]);

        assert!(!cfg.is_command_admin_only("help"));
        assert!(!cfg.is_command_admin_only("config"));
    }

    #[test]
    fn rejects_an_unknown_exempt_command() {
        let err = access(true, &["balance", "balanse"])
            .validate()
            .unwrap_err();

        assert!(err.to_string().contains("`balanse`"), "{err}");
        assert!(err.to_string().contains("balance, topup"), "{err}");
    }
}

fn tron_user(private_key: Option<&str>, seed_phrase: Option<&str>) -> ConfigUser {
    let mut user = base_user();
    user.tron = Some(ConfigUserTron {
        private_key: private_key.map(str::to_owned),
        seed_phrase: seed_phrase.map(str::to_owned),
        origin: None,
    });
    user
}

// A valid secp256k1 key; the standard BIP-39 test phrase.
const TRON_PRIVATE_KEY: &str = "b5a4cea271ff424d7c31dc12a3e43e401df7a40d7412a15750f3f0b6b5449a28";
const TRON_SEED_PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

#[cfg(feature = "tron-login")]
#[test]
fn auth_config_uses_tron_mode_with_a_private_key() {
    let user = tron_user(Some(TRON_PRIVATE_KEY), None);

    let auth = user
        .auth_config("example.com")
        .expect("tron auth should be valid");

    match auth {
        ConfigUserAuth::Tron {
            username,
            user_id,
            key,
            origin,
        } => {
            assert_eq!(username, "baibot");
            assert_eq!(user_id.as_str(), "@baibot:example.com");
            assert!(matches!(key, TronKeySource::PrivateKeyHex(ref k) if k == TRON_PRIVATE_KEY));
            assert_eq!(origin, "https://baibot.tron.mx");
        }
        other => panic!("expected tron auth mode, got {other:?}"),
    }
}

#[cfg(feature = "tron-login")]
#[test]
fn auth_config_uses_tron_mode_with_a_seed_phrase_and_custom_origin() {
    let mut user = tron_user(None, Some(TRON_SEED_PHRASE));
    user.tron.as_mut().unwrap().origin = Some("https://other.example".to_owned());

    let auth = user
        .auth_config("example.com")
        .expect("tron auth should be valid");

    match auth {
        ConfigUserAuth::Tron { key, origin, .. } => {
            assert!(matches!(key, TronKeySource::SeedPhrase(ref s) if s == TRON_SEED_PHRASE));
            assert_eq!(origin, "https://other.example");
        }
        other => panic!("expected tron auth mode, got {other:?}"),
    }
}

#[cfg(feature = "tron-login")]
#[test]
fn auth_config_rejects_an_unusable_tron_key() {
    let user = tron_user(Some("not-a-key"), None);

    let err = user
        .auth_config("example.com")
        .expect_err("a malformed key should be rejected");

    assert!(err.to_string().contains("user.tron"));
    assert!(err.to_string().contains("64 hex characters"));
}

#[cfg(not(feature = "tron-login"))]
#[test]
fn auth_config_rejects_tron_mode_without_the_feature() {
    let user = tron_user(Some(TRON_PRIVATE_KEY), None);

    let err = user
        .auth_config("example.com")
        .expect_err("tron auth needs the feature");

    assert!(err.to_string().contains("tron-login"));
}

#[test]
fn auth_config_rejects_tron_with_both_keys() {
    let user = tron_user(Some(TRON_PRIVATE_KEY), Some(TRON_SEED_PHRASE));

    let err = user
        .auth_config("example.com")
        .expect_err("two keys should be rejected");

    assert!(
        err.to_string()
            .contains("exactly one of user.tron.private_key")
    );
}

#[test]
fn auth_config_rejects_tron_without_a_key() {
    let user = tron_user(None, Some(""));

    let err = user
        .auth_config("example.com")
        .expect_err("a keyless section should be rejected");

    assert!(err.to_string().contains("needs the wallet key"));
}

#[test]
fn auth_config_rejects_tron_together_with_a_password() {
    let mut user = tron_user(Some(TRON_PRIVATE_KEY), None);
    user.password = Some("secret".to_owned());

    let err = user
        .auth_config("example.com")
        .expect_err("two auth modes should be rejected");

    assert!(
        err.to_string()
            .contains("exactly one authentication method")
    );
}

#[test]
fn tron_section_debug_output_hides_the_key_material() {
    let user = tron_user(Some(TRON_PRIVATE_KEY), Some(TRON_SEED_PHRASE));
    let debug = format!("{:?}", user.tron.as_ref().unwrap());

    assert!(debug.contains("[redacted]"));
    assert!(!debug.contains(TRON_PRIVATE_KEY));
    assert!(!debug.contains("abandon"));
}
