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
//! 2. Edit the constants below.
//! 3. Bump `PRICING_TABLE_REVISION` so log lines that include it can be
//!    correlated with the on-chain charge later.

#[derive(Debug, Clone, Copy)]
pub struct ModelPrice {
    /// USD per single input token.
    pub input_per_token: f64,
    /// USD per single output token.
    pub output_per_token: f64,
}

/// Bump when entries change. Logged into `meta.pricing_revision` of any
/// `charge` row that used fallback pricing.
pub const PRICING_TABLE_REVISION: &str = "2026-05-24";

/// Static lookup. Add new models here as we onboard them.
/// All prices are USD per token (NOT per million — divide by 1e6 from the
/// OpenRouter values).
pub fn lookup(model_id: &str) -> Option<ModelPrice> {
    Some(match model_id {
        // From openrouter.ai/api/v1/models on 2026-05-02:
        // minimax-m2.7-20260318: $0.0000003 / $0.0000012 per token
        "minimax/minimax-m2.7-20260318" => ModelPrice {
            input_per_token: 0.000_000_3,
            output_per_token: 0.000_001_2,
        },
        // minimax-m2.5-20260211: $0.00000015 / $0.00000115 per token
        "minimax/minimax-m2.5-20260211" => ModelPrice {
            input_per_token: 0.000_000_15,
            output_per_token: 0.000_001_15,
        },
        // Free tier — zero cost, but keep the entry so token usage still
        // gets accounted (could matter for cap counting if we ever bill 0%).
        "minimax/minimax-m2.5:free-20260211" => ModelPrice {
            input_per_token: 0.0,
            output_per_token: 0.0,
        },
        // From openrouter.ai/api/v1/models on 2026-05-24:
        // x-ai/grok-4.20: $0.00000125 / $0.0000025 per token.
        // OpenRouter does NOT return `usage.cost` for this model, so this
        // fallback row is the primary pricing path — without it every
        // chat reply is silently charged $0 and the "unknown_model_audit"
        // meta flag fires.
        "x-ai/grok-4.20" | "x-ai/grok-4.20-20260309" => ModelPrice {
            input_per_token: 0.000_001_25,
            output_per_token: 0.000_002_5,
        },
        _ => return None,
    })
}

/// Compute USD cost for a (prompt, completion) token pair using the
/// fallback table. Returns `None` if the model is unknown.
pub fn compute_fallback(model_id: &str, prompt_tokens: u32, completion_tokens: u32) -> Option<f64> {
    let p = lookup(model_id)?;
    Some(
        p.input_per_token * (prompt_tokens as f64)
            + p.output_per_token * (completion_tokens as f64),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_computes_cost() {
        let cost = compute_fallback("minimax/minimax-m2.7-20260318", 1000, 500).unwrap();
        // 1000 * 0.0000003 + 500 * 0.0000012 = 0.0003 + 0.0006 = 0.0009
        assert!((cost - 0.0009).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn free_tier_costs_zero() {
        let cost = compute_fallback("minimax/minimax-m2.5:free-20260211", 5000, 5000).unwrap();
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(compute_fallback("acme/whatever", 100, 50).is_none());
    }

    #[test]
    fn grok_4_20_pricing_matches_openrouter_table() {
        let cost = compute_fallback("x-ai/grok-4.20", 1000, 500).unwrap();
        // 1000 * 0.00000125 + 500 * 0.0000025 = 0.00125 + 0.00125 = 0.0025
        assert!((cost - 0.0025).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn grok_4_20_dated_alias_resolves_to_same_price() {
        let bare = compute_fallback("x-ai/grok-4.20", 100, 100).unwrap();
        let dated = compute_fallback("x-ai/grok-4.20-20260309", 100, 100).unwrap();
        assert!((bare - dated).abs() < 1e-12);
    }
}
