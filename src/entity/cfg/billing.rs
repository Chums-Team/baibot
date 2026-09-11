use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::billing::{BillingWrapperConfig, ModelPrice, PricingTable};

/// Static configuration of the optional billing feature.
///
/// When the `billing` section is absent from the configuration, the bot runs without
/// billing, exactly like upstream baibot. When present, every field has a default, so
/// an empty `billing: {}` section enables billing with the built-in policy.
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigBilling {
    /// Amount (USD) held off the room balance for the duration of a single LLM call.
    #[serde(default = "super::defaults::billing_reserve_amount_usd")]
    pub reserve_amount_usd: f64,

    /// Multiplier applied to the provider's cost. `1.0` charges the cost as is, `2.0` charges double.
    #[serde(default = "super::defaults::billing_markup_pct")]
    pub markup_pct: f64,

    /// Bot-wide (all rooms) spending caps per UTC calendar day and month.
    #[serde(default = "super::defaults::billing_daily_cap_usd")]
    pub daily_cap_usd: f64,

    #[serde(default = "super::defaults::billing_monthly_cap_usd")]
    pub monthly_cap_usd: f64,

    /// Bounds for a single top-up request from a user.
    #[serde(default = "super::defaults::billing_min_topup_usd")]
    pub min_topup_usd: f64,

    #[serde(default = "super::defaults::billing_max_topup_usd")]
    pub max_topup_usd: f64,

    /// Full Matrix user ids allowed to run billing administration commands.
    /// Independent of `access.admin_patterns`.
    #[serde(default)]
    pub admin_mxids: Vec<String>,

    /// Path to the SQLite ledger file. Defaults to `billing.db` inside `persistence.data_dir_path`.
    #[serde(default)]
    pub db_path: Option<String>,

    /// Per-model prices used when the provider does not report the cost of a call.
    /// Entries add to (or override) the built-in table. Keyed by model id.
    #[serde(default)]
    pub pricing: HashMap<String, ConfigBillingModelPrice>,
}

impl Default for ConfigBilling {
    fn default() -> Self {
        Self {
            reserve_amount_usd: super::defaults::billing_reserve_amount_usd(),
            markup_pct: super::defaults::billing_markup_pct(),
            daily_cap_usd: super::defaults::billing_daily_cap_usd(),
            monthly_cap_usd: super::defaults::billing_monthly_cap_usd(),
            min_topup_usd: super::defaults::billing_min_topup_usd(),
            max_topup_usd: super::defaults::billing_max_topup_usd(),
            admin_mxids: Vec::new(),
            db_path: None,
            pricing: HashMap::new(),
        }
    }
}

/// Prices are given per million tokens, the way providers publish them.
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigBillingModelPrice {
    pub input_usd_per_million_tokens: f64,
    pub output_usd_per_million_tokens: f64,
}

impl ConfigBilling {
    pub fn validate(&self) -> anyhow::Result<()> {
        for (name, value, env) in [
            (
                "reserve_amount_usd",
                self.reserve_amount_usd,
                super::env::BAIBOT_BILLING_RESERVE_AMOUNT_USD,
            ),
            (
                "markup_pct",
                self.markup_pct,
                super::env::BAIBOT_BILLING_MARKUP_PCT,
            ),
            (
                "daily_cap_usd",
                self.daily_cap_usd,
                super::env::BAIBOT_BILLING_DAILY_CAP_USD,
            ),
            (
                "monthly_cap_usd",
                self.monthly_cap_usd,
                super::env::BAIBOT_BILLING_MONTHLY_CAP_USD,
            ),
            (
                "min_topup_usd",
                self.min_topup_usd,
                super::env::BAIBOT_BILLING_MIN_TOPUP_USD,
            ),
            (
                "max_topup_usd",
                self.max_topup_usd,
                super::env::BAIBOT_BILLING_MAX_TOPUP_USD,
            ),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(anyhow::anyhow!(
                    "The billing.{name} ({env}) configuration must be a positive number, got {value}",
                ));
            }
        }

        if self.min_topup_usd > self.max_topup_usd {
            return Err(anyhow::anyhow!(
                "The billing.min_topup_usd ({}) configuration must not exceed billing.max_topup_usd ({})",
                super::env::BAIBOT_BILLING_MIN_TOPUP_USD,
                super::env::BAIBOT_BILLING_MAX_TOPUP_USD,
            ));
        }

        for mxid in &self.admin_mxids {
            if mxlink::matrix_sdk::ruma::UserId::parse(mxid).is_err() {
                return Err(anyhow::anyhow!(
                    "The billing.admin_mxids ({}) configuration contains an invalid Matrix user id: {}",
                    super::env::BAIBOT_BILLING_ADMIN_MXIDS,
                    mxid,
                ));
            }
        }

        for (model_id, price) in &self.pricing {
            for (name, value) in [
                (
                    "input_usd_per_million_tokens",
                    price.input_usd_per_million_tokens,
                ),
                (
                    "output_usd_per_million_tokens",
                    price.output_usd_per_million_tokens,
                ),
            ] {
                if !value.is_finite() || value < 0.0 {
                    return Err(anyhow::anyhow!(
                        "The billing.pricing.{model_id}.{name} configuration must be a non-negative number, got {value}",
                    ));
                }
            }
        }

        Ok(())
    }

    pub fn is_admin(&self, mxid: &str) -> bool {
        self.admin_mxids.iter().any(|admin| admin == mxid)
    }

    /// Where the ledger lives. Explicit `db_path` wins; otherwise `billing.db` inside the
    /// persistence data directory.
    pub fn db_path(&self, persistence: &super::PersistenceConfig) -> anyhow::Result<PathBuf> {
        if let Some(path) = &self.db_path {
            return Ok(PathBuf::from(path));
        }

        let mut path = persistence.data_dir_path_or_err()?;
        path.push(super::defaults::billing_db_file_name());
        Ok(path)
    }

    pub fn wrapper_config(&self) -> BillingWrapperConfig {
        BillingWrapperConfig {
            reserve_amount_usd: self.reserve_amount_usd,
            markup_pct: self.markup_pct,
            daily_cap_usd: self.daily_cap_usd,
            monthly_cap_usd: self.monthly_cap_usd,
        }
    }

    /// The built-in pricing table with the configured entries applied on top.
    pub fn pricing_table(&self) -> PricingTable {
        PricingTable::builtin().with_overrides(self.pricing.iter().map(|(model_id, price)| {
            (
                model_id.clone(),
                ModelPrice {
                    input_per_token: price.input_usd_per_million_tokens / 1_000_000.0,
                    output_per_token: price.output_usd_per_million_tokens / 1_000_000.0,
                },
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::cfg::defaults;

    fn parse(yaml: &str) -> ConfigBilling {
        serde_yaml_ng::from_str(yaml).expect("valid billing yaml")
    }

    fn persistence(data_dir_path: Option<&str>) -> super::super::PersistenceConfig {
        serde_yaml_ng::from_str(&match data_dir_path {
            Some(path) => format!("data_dir_path: {path}\n"),
            None => "data_dir_path: null\n".to_owned(),
        })
        .expect("valid persistence yaml")
    }

    #[test]
    fn empty_section_uses_defaults_and_validates() {
        let billing = parse("{}");
        assert_eq!(
            billing.reserve_amount_usd,
            defaults::billing_reserve_amount_usd()
        );
        assert_eq!(billing.markup_pct, defaults::billing_markup_pct());
        assert_eq!(billing.daily_cap_usd, defaults::billing_daily_cap_usd());
        assert_eq!(billing.monthly_cap_usd, defaults::billing_monthly_cap_usd());
        assert_eq!(billing.min_topup_usd, defaults::billing_min_topup_usd());
        assert_eq!(billing.max_topup_usd, defaults::billing_max_topup_usd());
        assert!(billing.admin_mxids.is_empty());
        assert!(billing.db_path.is_none());
        assert!(billing.pricing.is_empty());
        billing.validate().expect("defaults are valid");
    }

    #[test]
    fn full_section_parses() {
        let billing = parse(
            r#"
reserve_amount_usd: 0.05
markup_pct: 1.5
daily_cap_usd: 10
monthly_cap_usd: 100
min_topup_usd: 0.5
max_topup_usd: 5
admin_mxids:
  - "@alice:example.com"
db_path: /var/lib/baibot/billing.db
pricing:
  acme/model:
    input_usd_per_million_tokens: 1.25
    output_usd_per_million_tokens: 2.5
"#,
        );
        billing.validate().expect("valid");
        assert_eq!(billing.markup_pct, 1.5);
        assert_eq!(billing.admin_mxids, vec!["@alice:example.com".to_owned()]);
        assert_eq!(
            billing.db_path.as_deref(),
            Some("/var/lib/baibot/billing.db")
        );
        assert!(billing.is_admin("@alice:example.com"));
        assert!(!billing.is_admin("@bob:example.com"));
    }

    #[test]
    fn non_positive_numbers_are_rejected() {
        for yaml in [
            "reserve_amount_usd: 0",
            "markup_pct: -1",
            "daily_cap_usd: 0",
            "monthly_cap_usd: -5",
            "min_topup_usd: 0",
            "max_topup_usd: 0",
        ] {
            let err = parse(yaml).validate().expect_err(yaml).to_string();
            assert!(err.contains("must be a positive number"), "{yaml}: {err}");
        }
    }

    #[test]
    fn min_topup_above_max_is_rejected() {
        let err = parse("min_topup_usd: 2\nmax_topup_usd: 1")
            .validate()
            .expect_err("min > max")
            .to_string();
        assert!(err.contains("must not exceed"), "{err}");
    }

    #[test]
    fn invalid_admin_mxid_is_rejected() {
        let err = parse("admin_mxids: [\"not-an-mxid\"]")
            .validate()
            .expect_err("invalid mxid")
            .to_string();
        assert!(err.contains("invalid Matrix user id"), "{err}");
    }

    #[test]
    fn negative_price_is_rejected_but_zero_is_allowed() {
        let err = parse(
            "pricing:\n  acme/model:\n    input_usd_per_million_tokens: -1\n    output_usd_per_million_tokens: 0",
        )
        .validate()
        .expect_err("negative price")
        .to_string();
        assert!(err.contains("non-negative"), "{err}");

        parse(
            "pricing:\n  acme/free:\n    input_usd_per_million_tokens: 0\n    output_usd_per_million_tokens: 0",
        )
        .validate()
        .expect("zero price is a valid free tier");
    }

    #[test]
    fn pricing_table_converts_per_million_to_per_token() {
        let billing = parse(
            "pricing:\n  acme/model:\n    input_usd_per_million_tokens: 1.25\n    output_usd_per_million_tokens: 2.5",
        );
        let table = billing.pricing_table();
        let price = table.lookup("acme/model").expect("configured entry");
        assert!((price.input_per_token - 0.000_001_25).abs() < 1e-15);
        assert!((price.output_per_token - 0.000_002_5).abs() < 1e-15);
        assert!(table.revision().ends_with("+config"));
        // Built-in entries remain available.
        assert!(table.lookup("x-ai/grok-4.20").is_some());
    }

    #[test]
    fn wrapper_config_mirrors_policy_fields() {
        let billing =
            parse("reserve_amount_usd: 0.07\nmarkup_pct: 3\ndaily_cap_usd: 1\nmonthly_cap_usd: 2");
        let wrapper = billing.wrapper_config();
        assert_eq!(wrapper.reserve_amount_usd, 0.07);
        assert_eq!(wrapper.markup_pct, 3.0);
        assert_eq!(wrapper.daily_cap_usd, 1.0);
        assert_eq!(wrapper.monthly_cap_usd, 2.0);
    }

    #[test]
    fn db_path_prefers_explicit_value() {
        let billing = parse("db_path: /elsewhere/ledger.db");
        let path = billing
            .db_path(&persistence(None))
            .expect("explicit path needs no data dir");
        assert_eq!(path, PathBuf::from("/elsewhere/ledger.db"));
    }

    #[test]
    fn db_path_defaults_into_persistence_data_dir() {
        let billing = parse("{}");
        let path = billing
            .db_path(&persistence(Some("/data")))
            .expect("data dir set");
        assert_eq!(path, PathBuf::from("/data/billing.db"));

        let err = billing
            .db_path(&persistence(None))
            .expect_err("no data dir");
        assert!(
            err.to_string().contains("persistence.data_dir_path"),
            "{err}"
        );
    }
}
