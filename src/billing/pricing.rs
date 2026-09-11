//! Fallback pricing for OpenRouter models.
//!
//! OpenRouter normally returns the actual cost in `usage.cost` (USD) on
//! `/api/v1/chat/completions` responses. Our `text_generation` wrapper
//! prefers that field. **This table is a safety net** for the case
//! where `usage.cost` is missing or zero — e.g. a free-tier model, an
//! older API path, or a transient bug. We compute cost from token counts
//! using the per-million-token rates we recorded from
//! `https://openrouter.ai/api/v1/models` at table-update time.
//!
//! Update procedure when prices drift:
//! 1. `curl https://openrouter.ai/api/v1/models | jq '.data[] | select(.id | contains("minimax"))'`
//! 2. Edit the entries in `PricingTable::builtin` below (or add an entry to the
//!    `billing.pricing` section of the configuration without rebuilding).
//! 3. Bump `PRICING_TABLE_REVISION` so log lines that include it can be
//!    correlated with the on-chain charge later.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    /// USD per single input token.
    pub input_per_token: f64,
    /// USD per single output token.
    pub output_per_token: f64,
}

/// Bump when the built-in entries change. Logged into `meta.pricing_revision` of any
/// `charge` row that used fallback pricing.
pub const PRICING_TABLE_REVISION: &str = "2026-05-24";

/// Suffix appended to the revision when the configuration adds or overrides entries.
const CONFIG_REVISION_SUFFIX: &str = "+config";

/// Per-model fallback prices: the built-in table, optionally extended from the configuration.
#[derive(Debug, Clone)]
pub struct PricingTable {
    entries: HashMap<String, ModelPrice>,
    revision: String,
}

impl PricingTable {
    /// The built-in table.
    /// All prices are USD per token (NOT per million — divide by 1e6 from the provider values).
    pub fn builtin() -> Self {
        let mut entries = HashMap::new();

        // From openrouter.ai/api/v1/models on 2026-05-02:
        // minimax-m2.7-20260318: $0.0000003 / $0.0000012 per token
        entries.insert(
            "minimax/minimax-m2.7-20260318".to_owned(),
            ModelPrice {
                input_per_token: 0.000_000_3,
                output_per_token: 0.000_001_2,
            },
        );
        // minimax-m2.5-20260211: $0.00000015 / $0.00000115 per token
        entries.insert(
            "minimax/minimax-m2.5-20260211".to_owned(),
            ModelPrice {
                input_per_token: 0.000_000_15,
                output_per_token: 0.000_001_15,
            },
        );
        // Free tier — zero cost, but keep the entry so token usage still
        // gets accounted (could matter for cap counting if we ever bill 0%).
        entries.insert(
            "minimax/minimax-m2.5:free-20260211".to_owned(),
            ModelPrice {
                input_per_token: 0.0,
                output_per_token: 0.0,
            },
        );
        // From openrouter.ai/api/v1/models on 2026-05-24:
        // x-ai/grok-4.20: $0.00000125 / $0.0000025 per token.
        // OpenRouter does NOT return `usage.cost` for this model, so this
        // fallback row is the primary pricing path — without it every
        // chat reply is silently charged $0.
        for model_id in ["x-ai/grok-4.20", "x-ai/grok-4.20-20260309"] {
            entries.insert(
                model_id.to_owned(),
                ModelPrice {
                    input_per_token: 0.000_001_25,
                    output_per_token: 0.000_002_5,
                },
            );
        }

        Self {
            entries,
            revision: PRICING_TABLE_REVISION.to_owned(),
        }
    }

    /// Adds entries on top of the table, replacing built-in ones with the same model id.
    /// Any override marks the revision with a `+config` suffix so ledger rows can tell
    /// configured prices from built-in ones.
    pub fn with_overrides(
        mut self,
        overrides: impl IntoIterator<Item = (String, ModelPrice)>,
    ) -> Self {
        let mut overridden = false;
        for (model_id, price) in overrides {
            self.entries.insert(model_id, price);
            overridden = true;
        }
        if overridden && !self.revision.ends_with(CONFIG_REVISION_SUFFIX) {
            self.revision.push_str(CONFIG_REVISION_SUFFIX);
        }
        self
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn lookup(&self, model_id: &str) -> Option<ModelPrice> {
        self.entries.get(model_id).copied()
    }

    /// Compute USD cost for a (prompt, completion) token pair.
    /// Returns `None` if the model is unknown.
    pub fn compute_fallback(
        &self,
        model_id: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) -> Option<f64> {
        let p = self.lookup(model_id)?;
        Some(
            p.input_per_token * (prompt_tokens as f64)
                + p.output_per_token * (completion_tokens as f64),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_computes_cost() {
        let cost = PricingTable::builtin()
            .compute_fallback("minimax/minimax-m2.7-20260318", 1000, 500)
            .unwrap();
        // 1000 * 0.0000003 + 500 * 0.0000012 = 0.0003 + 0.0006 = 0.0009
        assert!((cost - 0.0009).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn free_tier_costs_zero() {
        let cost = PricingTable::builtin()
            .compute_fallback("minimax/minimax-m2.5:free-20260211", 5000, 5000)
            .unwrap();
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(
            PricingTable::builtin()
                .compute_fallback("acme/whatever", 100, 50)
                .is_none()
        );
    }

    #[test]
    fn grok_4_20_pricing_matches_openrouter_table() {
        let cost = PricingTable::builtin()
            .compute_fallback("x-ai/grok-4.20", 1000, 500)
            .unwrap();
        // 1000 * 0.00000125 + 500 * 0.0000025 = 0.00125 + 0.00125 = 0.0025
        assert!((cost - 0.0025).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn grok_4_20_dated_alias_resolves_to_same_price() {
        let bare = PricingTable::builtin()
            .compute_fallback("x-ai/grok-4.20", 100, 100)
            .unwrap();
        let dated = PricingTable::builtin()
            .compute_fallback("x-ai/grok-4.20-20260309", 100, 100)
            .unwrap();
        assert!((bare - dated).abs() < 1e-12);
    }

    #[test]
    fn override_replaces_builtin_entry_and_marks_revision() {
        let table = PricingTable::builtin().with_overrides([(
            "x-ai/grok-4.20".to_owned(),
            ModelPrice {
                input_per_token: 0.000_002,
                output_per_token: 0.000_004,
            },
        )]);
        let cost = table.compute_fallback("x-ai/grok-4.20", 1000, 500).unwrap();
        // 1000 * 0.000002 + 500 * 0.000004 = 0.002 + 0.002 = 0.004
        assert!((cost - 0.004).abs() < 1e-9, "got {cost}");
        assert_eq!(table.revision(), format!("{PRICING_TABLE_REVISION}+config"));
        // Untouched entries are still there.
        assert!(table.lookup("minimax/minimax-m2.7-20260318").is_some());
    }

    #[test]
    fn override_adds_unknown_model() {
        let table = PricingTable::builtin().with_overrides([(
            "acme/new-model".to_owned(),
            ModelPrice {
                input_per_token: 0.000_001,
                output_per_token: 0.000_001,
            },
        )]);
        let cost = table.compute_fallback("acme/new-model", 100, 100).unwrap();
        assert!((cost - 0.0002).abs() < 1e-12, "got {cost}");
    }

    #[test]
    fn no_overrides_keeps_builtin_revision() {
        let table = PricingTable::builtin().with_overrides(std::iter::empty());
        assert_eq!(table.revision(), PRICING_TABLE_REVISION);
    }
}
