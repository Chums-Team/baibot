//! Localization of the bot's own replies.
//!
//! Translations live in `locales/<code>.yml` (one file per locale, `rust-i18n` format, `%{name}`
//! placeholders) and are embedded into the binary at compile time by the `rust_i18n::i18n!` call
//! in `lib.rs`. A reply is rendered with `rust_i18n::t!("key", locale = ..., name = value)`.
//!
//! Only the replies added for the Chums client are localized (billing, top-ups). Upstream
//! messages (help, configuration, provider errors) stay English.
//!
//! ## Which locale a reply uses
//!
//! Matrix account data is private to its user, so the bot cannot read a user's language
//! preference. Instead, the client declares it by sending a `cc.chums.set_user_locale` event
//! into a room shared with the bot (see [`crate::matrix::events::SetUserLocaleContent`]). The
//! bot keeps the latest declared locale per user in memory ([`UserLocaleManager`]); users who
//! have not declared one get `i18n.fallback_locale` from the configuration. The map is empty
//! after a restart until clients send their locale again.
//!
//! A key missing from a locale falls back to [`DEFAULT_LOCALE`], so a partially translated
//! locale still works. The tests in this module check that no locale is partial.

#[cfg(test)]
mod tests;
mod user_locale;

pub use user_locale::UserLocaleManager;

/// The locale every key is guaranteed to exist in. Also the default of `i18n.fallback_locale`
/// and the last resort of `t!` (the `fallback` of the `rust_i18n::i18n!` call in `lib.rs`).
pub const DEFAULT_LOCALE: &str = "en";

/// The locales with a translation file, sorted.
pub fn available_locales() -> Vec<String> {
    rust_i18n::available_locales!()
        .into_iter()
        .map(|locale| locale.into_owned())
        .collect()
}

/// Maps a locale declared by a client onto one of the [`available_locales`].
///
/// Accepts `ru`, `pt-BR`, `pt_BR`, ` DE ` and the like. A regional variant maps onto its
/// language when only the language has translations. Returns `None` for an empty value and for
/// a language without translations.
pub fn normalize_locale(declared: &str) -> Option<String> {
    let locale = declared.trim().replace('_', "-").to_ascii_lowercase();
    if locale.is_empty() {
        return None;
    }

    let available = available_locales();

    if let Some(exact) = available.iter().find(|a| a.eq_ignore_ascii_case(&locale)) {
        return Some(exact.clone());
    }

    let language = locale.split('-').next()?;
    available
        .into_iter()
        .find(|a| a.eq_ignore_ascii_case(language))
}
