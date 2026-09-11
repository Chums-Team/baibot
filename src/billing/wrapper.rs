//! Pure logic for the LLM-call billing wrapper.
//!
//! This module is intentionally isolated from `matrix-sdk` and from the
//! upstream baibot `chat_completion` controller — it only takes plain
//! values and returns plain enums. The integration point in
//! `controller/chat_completion/mod.rs` is a thin glue layer that:
//!
//! 1. Reads the room/global config (markup, caps, reserve amount).
//! 2. Calls `pre_check` here. On any non-`AllowProceed` outcome — emits the
//!    matching `cc.chums.*` event and returns without invoking the LLM.
//! 3. Allocates `correlation_id`, calls `BillingService::reserve(...)`.
//! 4. Invokes the agent.
//! 5. On Ok — `compute_charge(usage.cost, ...)` then `BillingService::charge`
//!    + `release`.
//! 6. On Err — leaves the reserve as a zombie; `billing zombies`
//!    surfaces and resolves it via `manual_release`.
//!
//! The split keeps tests fast (no async fixtures, no matrix-sdk init) and
//! makes the wrapper logic provable independently of upstream churn.

use serde_json::json;
use uuid::Uuid;

use super::ledger::BillingService;
use super::pricing::PricingTable;
use super::types::{BillingError, Period};

/// Read from `global_config.billing` (or env defaults) at the call site.
#[derive(Debug, Clone)]
pub struct BillingWrapperConfig {
    /// Held off the room balance per LLM call. Default `$0.03`.
    pub reserve_amount_usd: f64,
    /// `1.0` = no markup; `2.0` = 2x. Per-room override applied before passing.
    pub markup_pct: f64,
    pub daily_cap_usd: f64,
    pub monthly_cap_usd: f64,
}

impl BillingWrapperConfig {
    pub const DEFAULT_RESERVE: f64 = 0.03;
    pub const DEFAULT_MARKUP: f64 = 2.0;
    pub const DEFAULT_DAILY_CAP: f64 = 50.0;
    pub const DEFAULT_MONTHLY_CAP: f64 = 1000.0;

    pub fn defaults() -> Self {
        Self {
            reserve_amount_usd: Self::DEFAULT_RESERVE,
            markup_pct: Self::DEFAULT_MARKUP,
            daily_cap_usd: Self::DEFAULT_DAILY_CAP,
            monthly_cap_usd: Self::DEFAULT_MONTHLY_CAP,
        }
    }
}

/// What should the bot do *before* invoking the LLM?
#[derive(Debug, Clone, PartialEq)]
pub enum PreCheckOutcome {
    /// Reserve passed. The wrapper should now call `BillingService::reserve`
    /// and proceed to invoke the LLM.
    AllowProceed,
    /// Room balance is below `reserve_amount_usd` — emit
    /// `cc.chums.x402_request` and return without LLM call.
    InsufficientBalance { current_balance_usd: f64 },
    /// Bot-wide daily cap would be exceeded by this call. Emit
    /// `cc.chums.cap_hit{period:daily}`.
    DailyCapHit { spent_usd: f64, cap_usd: f64 },
    /// Bot-wide monthly cap would be exceeded. Emit
    /// `cc.chums.cap_hit{period:monthly}`.
    MonthlyCapHit { spent_usd: f64, cap_usd: f64 },
}

/// Single decision point. **Order matters** — balance first (so the user
/// gets a useful "top up" message rather than "service paused"), then
/// daily, then monthly. Daily before monthly so the user sees the more
/// actionable boundary.
pub async fn pre_check(
    billing: &BillingService,
    room_id: &str,
    cfg: &BillingWrapperConfig,
) -> Result<PreCheckOutcome, BillingError> {
    let balance = billing.compute_balance(room_id).await?;
    if balance < cfg.reserve_amount_usd {
        return Ok(PreCheckOutcome::InsufficientBalance {
            current_balance_usd: balance,
        });
    }

    let daily = billing.compute_period_spent(Period::Day).await?;
    if daily + cfg.reserve_amount_usd > cfg.daily_cap_usd {
        return Ok(PreCheckOutcome::DailyCapHit {
            spent_usd: daily,
            cap_usd: cfg.daily_cap_usd,
        });
    }

    let monthly = billing.compute_period_spent(Period::Month).await?;
    if monthly + cfg.reserve_amount_usd > cfg.monthly_cap_usd {
        return Ok(PreCheckOutcome::MonthlyCapHit {
            spent_usd: monthly,
            cap_usd: cfg.monthly_cap_usd,
        });
    }

    Ok(PreCheckOutcome::AllowProceed)
}

/// What the wrapper writes into `BillingService::charge(...)` for a
/// successful LLM call. The caller serializes the relevant fields into
/// `meta` for audit.
#[derive(Debug, Clone, PartialEq)]
pub struct ChargeBreakdown {
    /// What OpenRouter (or the fallback table) said the call cost in USD.
    pub actual_cost_usd: f64,
    /// Effective markup at call time. Recorded in meta so future audits
    /// can reconstruct the math even after global markup changes.
    pub markup_pct: f64,
    /// `actual_cost_usd * markup_pct`. Always positive.
    pub charged_usd: f64,
    /// `true` if the OpenRouter response lacked `usage.cost` and we fell
    /// back to the per-token pricing table.
    pub used_fallback_pricing: bool,
    /// Pricing table revision (see `PricingTable::revision`). Useful when a
    /// price update changes ledger entries but old charges remain valid.
    pub pricing_revision: String,
}

impl ChargeBreakdown {
    /// Caller passes this to `BillingService::charge(corr_id, charged_usd, meta_json)`.
    pub fn to_meta(
        &self,
        model_id: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) -> serde_json::Value {
        json!({
            "model": model_id,
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "actual_cost_usd": self.actual_cost_usd,
            "markup_pct": self.markup_pct,
            "used_fallback_pricing": self.used_fallback_pricing,
            "pricing_revision": self.pricing_revision,
        })
    }
}

/// Resolve the cost of a single LLM call, applying markup.
///
/// Prefers OpenRouter's authoritative `usage.cost` field. Falls back to
/// `PricingTable::compute_fallback` when the field is missing or zero on a
/// non-free-tier model. Returns `None` when both sources fail (unknown
/// model + missing usage.cost) — caller should treat as a logic bug,
/// log loudly, and leave the reserve hanging for manual review.
pub fn compute_charge(
    pricing: &PricingTable,
    usage_cost_usd: Option<f64>,
    model_id: &str,
    prompt_tokens: u32,
    completion_tokens: u32,
    markup_pct: f64,
) -> Option<ChargeBreakdown> {
    // Treat zero `usage.cost` as missing for non-free models — OpenRouter
    // has historically returned 0 by mistake on rare paths. The free-tier
    // model legitimately has 0 cost, so we still fall back (fallback also
    // returns 0 for free, costing the user nothing — same outcome).
    let used_fallback;
    let actual = match usage_cost_usd {
        Some(c) if c > 0.0 => {
            used_fallback = false;
            c
        }
        _ => {
            used_fallback = true;
            pricing.compute_fallback(model_id, prompt_tokens, completion_tokens)?
        }
    };

    let charged = actual * markup_pct;
    Some(ChargeBreakdown {
        actual_cost_usd: actual,
        markup_pct,
        charged_usd: charged,
        used_fallback_pricing: used_fallback,
        pricing_revision: pricing.revision().to_owned(),
    })
}

/// Convenience tag — attached to logs so a single LLM call can be traced
/// across pre_check / reserve / openrouter / charge / release.
pub fn new_correlation_id() -> Uuid {
    Uuid::now_v7()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: &str = "!w:example.com";
    const USER: &str = "@u:example.com";

    fn cfg() -> BillingWrapperConfig {
        BillingWrapperConfig {
            reserve_amount_usd: 0.03,
            markup_pct: 2.0,
            daily_cap_usd: 0.50, // small cap for test
            monthly_cap_usd: 5.00,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn precheck_allows_when_funded_and_under_caps() {
        let svc = BillingService::open_in_memory().unwrap();
        svc.topup(ROOM, 1.0, Some(USER), serde_json::json!({}))
            .await
            .unwrap();
        let res = pre_check(&svc, ROOM, &cfg()).await.unwrap();
        assert_eq!(res, PreCheckOutcome::AllowProceed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn precheck_insufficient_balance_when_zero() {
        let svc = BillingService::open_in_memory().unwrap();
        let res = pre_check(&svc, ROOM, &cfg()).await.unwrap();
        match res {
            PreCheckOutcome::InsufficientBalance {
                current_balance_usd,
            } => {
                assert!(current_balance_usd.abs() < 1e-9);
            }
            other => panic!("expected InsufficientBalance, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn precheck_daily_cap_blocks_after_threshold() {
        let svc = BillingService::open_in_memory().unwrap();
        svc.topup(ROOM, 100.0, Some(USER), serde_json::json!({}))
            .await
            .unwrap();
        // Drive the daily-spent past the cap.
        let corr = Uuid::now_v7();
        svc.reserve(ROOM, corr, 0.03, None, None).await.unwrap();
        svc.charge(corr, 0.49, serde_json::json!({})).await.unwrap();
        svc.release(corr).await.unwrap();
        // daily_spent = 0.49; reserve 0.03; cap 0.50; 0.49 + 0.03 = 0.52 > 0.50.
        let res = pre_check(&svc, ROOM, &cfg()).await.unwrap();
        match res {
            PreCheckOutcome::DailyCapHit { spent_usd, cap_usd } => {
                assert!((spent_usd - 0.49).abs() < 1e-9);
                assert_eq!(cap_usd, 0.50);
            }
            other => panic!("expected DailyCapHit, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn precheck_monthly_cap_blocks_after_threshold() {
        // For this test we leave daily_cap loose and tighten monthly.
        let mut c = cfg();
        c.daily_cap_usd = 1000.0; // out of the way
        c.monthly_cap_usd = 0.50;
        let svc = BillingService::open_in_memory().unwrap();
        svc.topup(ROOM, 100.0, Some(USER), serde_json::json!({}))
            .await
            .unwrap();
        let corr = Uuid::now_v7();
        svc.reserve(ROOM, corr, 0.03, None, None).await.unwrap();
        svc.charge(corr, 0.49, serde_json::json!({})).await.unwrap();
        svc.release(corr).await.unwrap();
        let res = pre_check(&svc, ROOM, &c).await.unwrap();
        match res {
            PreCheckOutcome::MonthlyCapHit { spent_usd, cap_usd } => {
                assert!((spent_usd - 0.49).abs() < 1e-9);
                assert_eq!(cap_usd, 0.50);
            }
            other => panic!("expected MonthlyCapHit, got {other:?}"),
        }
    }

    #[test]
    fn compute_charge_prefers_openrouter_cost_when_present() {
        let r = compute_charge(
            &PricingTable::builtin(),
            Some(0.005),
            "minimax/minimax-m2.7-20260318",
            1000,
            500,
            2.0,
        )
        .expect("should resolve");
        assert!((r.actual_cost_usd - 0.005).abs() < 1e-12);
        assert!((r.charged_usd - 0.010).abs() < 1e-12);
        assert!(!r.used_fallback_pricing);
        assert_eq!(r.pricing_revision, crate::billing::PRICING_TABLE_REVISION);
    }

    #[test]
    fn compute_charge_records_real_openrouter_cost_in_breakdown() {
        // Explicit acceptance: when controller passes through
        // OpenRouter's authoritative `total_cost`, the breakdown
        // records `used_fallback_pricing=false` so audits can tell the
        // call did NOT estimate.
        let r = compute_charge(
            &PricingTable::builtin(),
            Some(0.0024),
            "minimax/minimax-m2.7-20260318",
            1234,
            567,
            2.0,
        )
        .expect("should resolve with real cost");
        assert!((r.actual_cost_usd - 0.0024).abs() < 1e-12);
        assert!(
            !r.used_fallback_pricing,
            "real cost path must NOT mark fallback"
        );
        // Markup applied verbatim.
        assert!((r.charged_usd - 0.0048).abs() < 1e-12);
        // Meta carries the real cost into the ledger.
        let meta = r.to_meta("minimax/minimax-m2.7-20260318", 1234, 567);
        assert_eq!(meta["actual_cost_usd"], 0.0024);
        assert_eq!(meta["used_fallback_pricing"], false);
        assert_eq!(meta["prompt_tokens"], 1234);
        assert_eq!(meta["completion_tokens"], 567);
    }

    #[test]
    fn compute_charge_falls_back_when_cost_missing() {
        let r = compute_charge(
            &PricingTable::builtin(),
            None,
            "minimax/minimax-m2.7-20260318",
            1000,
            500,
            2.0,
        )
        .expect("fallback table should resolve");
        // 1000 * 0.0000003 + 500 * 0.0000012 = 0.0009 ; * 2 = 0.0018
        assert!((r.actual_cost_usd - 0.0009).abs() < 1e-12);
        assert!((r.charged_usd - 0.0018).abs() < 1e-12);
        assert!(r.used_fallback_pricing);
    }

    #[test]
    fn compute_charge_falls_back_when_cost_zero_for_paid_model() {
        // Some OpenRouter responses have erroneously returned 0 — treat
        // as missing for paid models. Free model legitimately costs 0.
        let r = compute_charge(
            &PricingTable::builtin(),
            Some(0.0),
            "minimax/minimax-m2.7-20260318",
            100,
            50,
            2.0,
        )
        .expect("fallback should kick in");
        assert!(r.used_fallback_pricing);
        assert!(r.actual_cost_usd > 0.0);
    }

    #[test]
    fn compute_charge_returns_none_for_unknown_model_without_cost() {
        // Both sources fail — caller should bail out (don't charge anything,
        // log loudly, leave reserve hanging for manual review).
        assert!(
            compute_charge(&PricingTable::builtin(), None, "acme/nope", 100, 50, 2.0).is_none()
        );
    }

    #[test]
    fn compute_charge_returns_zero_for_free_model() {
        let r = compute_charge(
            &PricingTable::builtin(),
            None,
            "minimax/minimax-m2.5:free-20260211",
            10000,
            10000,
            2.0,
        )
        .expect("free model in table");
        assert_eq!(r.actual_cost_usd, 0.0);
        assert_eq!(r.charged_usd, 0.0);
        assert!(r.used_fallback_pricing);
    }

    #[test]
    fn breakdown_meta_has_audit_fields() {
        let b = ChargeBreakdown {
            actual_cost_usd: 0.001,
            markup_pct: 2.0,
            charged_usd: 0.002,
            used_fallback_pricing: false,
            pricing_revision: "test".into(),
        };
        let m = b.to_meta("minimax/minimax-m2.7-20260318", 100, 50);
        assert_eq!(m["model"], "minimax/minimax-m2.7-20260318");
        assert_eq!(m["prompt_tokens"], 100);
        assert_eq!(m["completion_tokens"], 50);
        assert_eq!(m["actual_cost_usd"], 0.001);
        assert_eq!(m["markup_pct"], 2.0);
        assert_eq!(m["used_fallback_pricing"], false);
        assert_eq!(m["pricing_revision"], "test");
    }

    #[test]
    fn correlation_id_is_unique() {
        let a = new_correlation_id();
        let b = new_correlation_id();
        assert_ne!(a, b);
    }

    #[test]
    fn defaults_match_plan() {
        let d = BillingWrapperConfig::defaults();
        assert_eq!(d.reserve_amount_usd, 0.03);
        assert_eq!(d.markup_pct, 2.0);
        assert_eq!(d.daily_cap_usd, 50.0);
        assert_eq!(d.monthly_cap_usd, 1000.0);
    }
}
