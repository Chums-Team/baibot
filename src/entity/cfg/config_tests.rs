use super::{Avatar, ConfigUser, ConfigUserAuth, ConfigUserEncryption};
use crate::entity::cfg::env;

fn base_user() -> ConfigUser {
    ConfigUser {
        mxid_localpart: "baibot".to_owned(),
        password: None,
        access_token: None,
        device_id: None,
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
        ConfigUserAuth::AccessToken { .. } => {
            panic!("expected password auth mode");
        }
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
        ConfigUserAuth::UserPassword { .. } => {
            panic!("expected access token auth mode");
        }
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
