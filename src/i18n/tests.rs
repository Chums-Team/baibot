//! Consistency of the translation files and the locale helpers.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use rust_i18n::t;

use super::{DEFAULT_LOCALE, available_locales, normalize_locale};

/// `locale → key → text`, read from `locales/*.yml` the way `rust_i18n::i18n!` does.
fn locale_files() -> BTreeMap<String, BTreeMap<String, String>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
    let mut files = BTreeMap::new();

    for entry in fs::read_dir(&dir).expect("locales/ exists") {
        let path = entry.expect("readable entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("yml") {
            continue;
        }
        let locale = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("utf-8 file name")
            .to_owned();
        let raw = fs::read_to_string(&path).expect("readable locale file");
        let parsed: BTreeMap<String, serde_yaml_ng::Value> =
            serde_yaml_ng::from_str(&raw).unwrap_or_else(|err| panic!("{}: {err}", path.display()));

        let mut texts = BTreeMap::new();
        for (key, value) in parsed {
            if key == "_version" {
                continue;
            }
            let serde_yaml_ng::Value::String(text) = value else {
                panic!("{}: `{key}` must be a string", path.display());
            };
            texts.insert(key, text);
        }
        files.insert(locale, texts);
    }

    files
}

fn placeholders(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find("%{") {
        let after = &rest[start + 2..];
        let end = after.find('}').expect("closed placeholder");
        found.insert(after[..end].to_owned());
        rest = &after[end + 1..];
    }
    found
}

#[test]
fn locale_files_are_the_available_locales() {
    let files: Vec<String> = locale_files().into_keys().collect();
    assert_eq!(files, available_locales());
    assert!(files.iter().any(|locale| locale == DEFAULT_LOCALE));
}

#[test]
fn every_locale_has_exactly_the_english_keys() {
    let files = locale_files();
    let english: BTreeSet<&String> = files[DEFAULT_LOCALE].keys().collect();
    assert!(!english.is_empty());

    for (locale, texts) in &files {
        let keys: BTreeSet<&String> = texts.keys().collect();
        let missing: Vec<_> = english.difference(&keys).collect();
        let extra: Vec<_> = keys.difference(&english).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{locale}: missing {missing:?}, extra {extra:?}"
        );
    }
}

#[test]
fn every_locale_uses_the_english_placeholders() {
    let files = locale_files();
    let english = &files[DEFAULT_LOCALE];

    for (locale, texts) in &files {
        for (key, text) in texts {
            assert_eq!(
                placeholders(text),
                placeholders(&english[key]),
                "{locale}: placeholders of `{key}` differ from en"
            );
        }
    }
}

#[test]
fn translations_render_with_arguments() {
    assert_eq!(
        t!(
            "billing.topup_amount_out_of_range",
            locale = "en",
            min = "0.10",
            max = "1.00"
        ),
        "The amount must be between $0.10 and $1.00."
    );
    assert_eq!(
        t!(
            "billing.topup_amount_out_of_range",
            locale = "ru",
            min = "0.10",
            max = "1.00"
        ),
        "Сумма должна быть от $0.10 до $1.00."
    );
}

#[test]
fn unknown_locale_falls_back_to_english() {
    assert_eq!(
        t!("billing.balance.column_type", locale = "xx"),
        t!("billing.balance.column_type", locale = "en")
    );
}

#[test]
fn regional_variant_falls_back_to_its_language() {
    assert_eq!(
        t!("billing.balance.column_type", locale = "pt-BR"),
        t!("billing.balance.column_type", locale = "pt")
    );
}

#[test]
fn normalize_locale_accepts_declared_forms() {
    assert_eq!(normalize_locale("ru").as_deref(), Some("ru"));
    assert_eq!(normalize_locale(" DE ").as_deref(), Some("de"));
    assert_eq!(normalize_locale("pt-BR").as_deref(), Some("pt"));
    assert_eq!(normalize_locale("pt_BR").as_deref(), Some("pt"));
    assert_eq!(normalize_locale("sr-Latn-RS").as_deref(), Some("sr"));
}

#[test]
fn normalize_locale_rejects_unknown_and_empty() {
    assert_eq!(normalize_locale(""), None);
    assert_eq!(normalize_locale("   "), None);
    assert_eq!(normalize_locale("xx"), None);
    assert_eq!(normalize_locale("xx-YY"), None);
}
