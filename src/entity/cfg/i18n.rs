use serde::Deserialize;

/// Localization of the bot's replies. See `docs/configuration/i18n.md`.
///
/// Always present: without an `i18n` section the defaults apply.
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigI18n {
    /// Locale of the replies to users whose client has not declared a locale
    /// (with a `cc.chums.set_user_locale` event). One of the locales under `locales/`.
    #[serde(default = "super::defaults::i18n_fallback_locale")]
    pub fallback_locale: String,
}

impl Default for ConfigI18n {
    fn default() -> Self {
        Self {
            fallback_locale: super::defaults::i18n_fallback_locale(),
        }
    }
}

impl ConfigI18n {
    pub fn validate(&self) -> anyhow::Result<()> {
        let available = crate::i18n::available_locales();

        if !available
            .iter()
            .any(|locale| locale == &self.fallback_locale)
        {
            return Err(anyhow::anyhow!(
                "The i18n.fallback_locale ({}) configuration must be one of the locales the bot has translations for ({}), got `{}`",
                super::env::BAIBOT_I18N_FALLBACK_LOCALE,
                available.join(", "),
                self.fallback_locale,
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> ConfigI18n {
        serde_yaml_ng::from_str(yaml).expect("valid yaml")
    }

    #[test]
    fn empty_section_defaults_to_english() {
        let cfg = parse("{}");
        assert_eq!(cfg.fallback_locale, crate::i18n::DEFAULT_LOCALE);
        cfg.validate().unwrap();
        assert_eq!(cfg.fallback_locale, ConfigI18n::default().fallback_locale);
    }

    #[test]
    fn any_available_locale_validates() {
        for locale in crate::i18n::available_locales() {
            parse(&format!("fallback_locale: {locale}"))
                .validate()
                .unwrap_or_else(|err| panic!("{locale}: {err}"));
        }
    }

    #[test]
    fn unknown_locale_is_rejected_listing_the_available_ones() {
        let err = parse("fallback_locale: xx")
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("i18n.fallback_locale"), "{err}");
        assert!(err.contains("`xx`"), "{err}");
        assert!(err.contains("en"), "{err}");
        assert!(err.contains("ru"), "{err}");
    }

    #[test]
    fn locale_must_match_a_file_exactly() {
        // The operator writes this value; no guessing from regional variants or case.
        assert!(parse("fallback_locale: pt-BR").validate().is_err());
        assert!(parse("fallback_locale: EN").validate().is_err());
    }
}
