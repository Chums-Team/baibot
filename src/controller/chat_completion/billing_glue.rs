//! Glue between `controller::chat_completion` and `billing::wrapper`.
//!
//! Keeps the matrix-sdk-aware logic in `mod.rs` thin and the billing decisions
//! testable in isolation. The handler calls a single entry point,
//! `run_billed_call`, which chains:
//!
//! 1. `pre_check` + `reserve`. On a non-allow outcome (insufficient balance,
//!    cap hit) it returns without invoking the LLM, describing what the caller
//!    should announce in the room.
//! 2. The LLM closure.
//! 3. On the Ok path: compute the charge (preferring the provider-reported
//!    cost, falling back to the pricing table) and write `charge` + `release`.
//!    On the Err path: leave the reserve as a zombie for review with the
//!    billing administration commands.
//!
//! When billing is not configured, the closure runs directly and nothing is
//! recorded.
//!
//! Token estimation: when the provider doesn't surface `prompt_tokens` /
//! `completion_tokens`, we estimate from string lengths using a
//! `~4 chars per token` rule of thumb. This matches OpenAI's documented
//! heuristic close enough for fallback pricing; it's wrong for other
//! tokenisers, but only by a constant factor. The charge meta records
//! `estimated_tokens = true` so audits can spot the estimation.

use serde_json::{Value, json};
use tracing::warn;
use uuid::Uuid;

use crate::agent::provider::TextGenerationResult;
use crate::billing::wrapper::{ChargeBreakdown, PreCheckOutcome, compute_charge, pre_check};
use crate::billing::{BillingContext, BillingError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapPeriod {
    Daily,
    Monthly,
}

impl CapPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            CapPeriod::Daily => "daily",
            CapPeriod::Monthly => "monthly",
        }
    }
}

impl From<CapPeriod> for crate::matrix::events::CapPeriod {
    fn from(period: CapPeriod) -> Self {
        match period {
            CapPeriod::Daily => Self::Daily,
            CapPeriod::Monthly => Self::Monthly,
        }
    }
}

/// What the chat-completion handler should do next after the billing pre-check.
#[derive(Debug, Clone, PartialEq)]
enum BillingDecision {
    /// Pre-check passed, reserve allocated. Caller proceeds to the LLM.
    Allow { correlation_id: Uuid },
    /// Balance below `reserve_amount_usd`. Caller asks the user to top up.
    InsufficientBalance {
        current_balance_usd: f64,
        reserve_amount_usd: f64,
    },
    /// Daily/monthly cap exceeded. Caller announces the pause.
    CapHit {
        period: CapPeriod,
        spent_usd: f64,
        cap_usd: f64,
    },
    /// `pre_check` or `reserve` itself failed (DB error). Treat as transient,
    /// surface a generic error in the room. Nothing is charged.
    Error(String),
}

/// Step 1: pre_check + reserve.
async fn pre_check_and_reserve(
    billing: &BillingContext,
    room_id: &str,
    user_mxid: &str,
    matrix_event_id: &str,
) -> BillingDecision {
    let cfg = &billing.wrapper_config;

    match pre_check(&billing.service, room_id, cfg).await {
        Ok(PreCheckOutcome::AllowProceed) => {
            let correlation_id = Uuid::now_v7();
            match billing
                .service
                .reserve(
                    room_id,
                    correlation_id,
                    cfg.reserve_amount_usd,
                    Some(user_mxid),
                    Some(matrix_event_id),
                )
                .await
            {
                Ok(_) => BillingDecision::Allow { correlation_id },
                Err(e) => {
                    warn!(error = %e, %room_id, "billing reserve failed");
                    BillingDecision::Error(format!("billing reserve failed: {e}"))
                }
            }
        }
        Ok(PreCheckOutcome::InsufficientBalance {
            current_balance_usd,
        }) => BillingDecision::InsufficientBalance {
            current_balance_usd,
            reserve_amount_usd: cfg.reserve_amount_usd,
        },
        Ok(PreCheckOutcome::DailyCapHit { spent_usd, cap_usd }) => BillingDecision::CapHit {
            period: CapPeriod::Daily,
            spent_usd,
            cap_usd,
        },
        Ok(PreCheckOutcome::MonthlyCapHit { spent_usd, cap_usd }) => BillingDecision::CapHit {
            period: CapPeriod::Monthly,
            spent_usd,
            cap_usd,
        },
        Err(e) => {
            warn!(error = %e, %room_id, "billing pre_check failed");
            BillingDecision::Error(format!("billing pre_check failed: {e}"))
        }
    }
}

/// Step 2 (Ok path): write `charge` + `release` rows. Returns the concrete
/// amounts so the caller can log/audit.
async fn settle_charge(
    billing: &BillingContext,
    correlation_id: Uuid,
    markup_pct: f64,
    model_id: &str,
    result: &TextGenerationResult,
) -> Result<SettleSummary, BillingError> {
    let usage = result.usage.clone().unwrap_or_default();

    // Tokens: prefer the provider-reported counts; otherwise estimate from the
    // response text (see the module-level comment).
    let prompt_tokens = usage.prompt_tokens.unwrap_or(0);
    let completion_tokens = usage
        .completion_tokens
        .unwrap_or_else(|| estimate_tokens(&result.text));

    let breakdown = compute_charge(
        &billing.pricing,
        usage.cost_usd,
        model_id,
        prompt_tokens,
        completion_tokens,
        markup_pct,
    );

    let breakdown = match breakdown {
        Some(b) => b,
        None => {
            warn!(
                %correlation_id,
                model_id,
                "compute_charge returned None (unknown model and no provider cost). \
                 Charging $0 and releasing the reserve; flagged in meta for audit."
            );
            // Fall back to a $0 charge with a marker, then release. The
            // alternative (leaving a zombie) would block the room from
            // chatting on a model the bot can't price.
            ChargeBreakdown {
                actual_cost_usd: 0.0,
                markup_pct,
                charged_usd: 0.0,
                used_fallback_pricing: true,
                pricing_revision: billing.pricing.revision().to_owned(),
            }
        }
    };

    let mut meta: Value = breakdown.to_meta(model_id, prompt_tokens, completion_tokens);
    if breakdown.actual_cost_usd == 0.0 && usage.cost_usd.is_none() {
        // Distinguish "free model" (zero cost) from "unknown model" (0 by fallback).
        meta["unknown_model_audit"] = json!(true);
    }
    if usage.prompt_tokens.is_none() && usage.completion_tokens.is_none() {
        meta["estimated_tokens"] = json!(true);
    }

    let charge_event = billing
        .service
        .charge(correlation_id, breakdown.charged_usd, meta)
        .await?;
    let release_event = billing.service.release(correlation_id).await?;

    Ok(SettleSummary {
        charged_usd: breakdown.charged_usd,
        actual_cost_usd: breakdown.actual_cost_usd,
        used_fallback_pricing: breakdown.used_fallback_pricing,
        charge_event_id: charge_event.event_id,
        release_event_id: release_event.event_id,
    })
}

#[derive(Debug, Clone)]
pub struct SettleSummary {
    pub charged_usd: f64,
    pub actual_cost_usd: f64,
    pub used_fallback_pricing: bool,
    pub charge_event_id: Uuid,
    pub release_event_id: Uuid,
}

/// `~4 chars per token` heuristic. Returns `0` for empty input. Used only for
/// fallback-pricing scaling, where being wrong by a constant factor is
/// acceptable; the meta records `estimated_tokens = true`.
fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count();
    ((chars as f64) / 4.0).ceil() as u32
}

/// Resolve the effective markup: a valid per-room override beats the global default.
fn effective_markup_pct(room_override: Option<f64>, default: f64) -> f64 {
    room_override
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(default)
}

/// Outcome of `run_billed_call`. Each variant maps 1:1 to a side effect the
/// caller performs (room reply, error markdown), so this seam stays
/// matrix-sdk-free and integration-testable.
#[derive(Debug)]
pub enum BilledCallOutcome {
    /// Billing is not configured. The LLM returned text; nothing was recorded.
    Unbilled {
        text_generation: TextGenerationResult,
    },
    /// LLM returned text and the charge/release pair landed.
    Success {
        text_generation: TextGenerationResult,
        correlation_id: Uuid,
        summary: SettleSummary,
    },
    /// LLM returned text but the charge/release writeback failed. Caller
    /// still sends the user the answer; the reserve stays as a zombie for
    /// manual reconciliation.
    SettleFailed {
        text_generation: TextGenerationResult,
        correlation_id: Uuid,
        error: BillingError,
    },
    /// `pre_check` saw a balance below the reserve floor. LLM not called.
    InsufficientBalance {
        current_balance_usd: f64,
        reserve_amount_usd: f64,
    },
    /// Daily/monthly cap exceeded. LLM not called.
    CapHit {
        period: CapPeriod,
        spent_usd: f64,
        cap_usd: f64,
    },
    /// The LLM closure returned an `Err`. With billing enabled, the reserve
    /// is intentionally left in the ledger as a zombie for manual
    /// reconciliation: a half-completed provider call may have cost real money.
    LlmError {
        /// `None` when billing is not configured.
        correlation_id: Option<Uuid>,
        error: anyhow::Error,
    },
    /// `pre_check` or `reserve` failed (DB error). Caller surfaces a generic
    /// "billing temporarily unavailable" reply. Nothing is charged.
    PreCheckError(String),
}

/// Run the full billing chain around an LLM call: `pre_check + reserve` →
/// `invoke_llm()` → `charge + release`. With `billing` absent, only the
/// closure runs.
///
/// The caller passes the LLM as a closure so this function never sees the
/// matrix-aware controller types. In production the closure is
/// `controller.generate_text(conversation, params)` wrapped in the
/// thinking-notice race.
pub async fn run_billed_call<F, Fut>(
    billing: Option<&BillingContext>,
    room_id: &str,
    user_mxid: &str,
    matrix_event_id: &str,
    room_markup_override_pct: Option<f64>,
    model_id: &str,
    invoke_llm: F,
) -> BilledCallOutcome
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<TextGenerationResult>>,
{
    let Some(billing) = billing else {
        return match invoke_llm().await {
            Ok(text_generation) => BilledCallOutcome::Unbilled { text_generation },
            Err(error) => BilledCallOutcome::LlmError {
                correlation_id: None,
                error,
            },
        };
    };

    let markup_pct =
        effective_markup_pct(room_markup_override_pct, billing.wrapper_config.markup_pct);

    let decision = pre_check_and_reserve(billing, room_id, user_mxid, matrix_event_id).await;
    let correlation_id = match decision {
        BillingDecision::Allow { correlation_id } => correlation_id,
        BillingDecision::InsufficientBalance {
            current_balance_usd,
            reserve_amount_usd,
        } => {
            return BilledCallOutcome::InsufficientBalance {
                current_balance_usd,
                reserve_amount_usd,
            };
        }
        BillingDecision::CapHit {
            period,
            spent_usd,
            cap_usd,
        } => {
            return BilledCallOutcome::CapHit {
                period,
                spent_usd,
                cap_usd,
            };
        }
        BillingDecision::Error(msg) => return BilledCallOutcome::PreCheckError(msg),
    };

    let text_generation = match invoke_llm().await {
        Ok(r) => r,
        Err(e) => {
            return BilledCallOutcome::LlmError {
                correlation_id: Some(correlation_id),
                error: e,
            };
        }
    };

    match settle_charge(
        billing,
        correlation_id,
        markup_pct,
        model_id,
        &text_generation,
    )
    .await
    {
        Ok(summary) => BilledCallOutcome::Success {
            text_generation,
            correlation_id,
            summary,
        },
        Err(error) => BilledCallOutcome::SettleFailed {
            text_generation,
            correlation_id,
            error,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::billing::types::BillingEventType;
    use crate::billing::{BillingService, BillingWrapperConfig, PricingTable};

    const ROOM: &str = "!glue:example.com";
    const USER: &str = "@u:example.com";
    const EVENT: &str = "$abc";
    const MODEL: &str = "minimax/minimax-m2.7-20260318";

    fn cfg() -> BillingWrapperConfig {
        BillingWrapperConfig {
            reserve_amount_usd: 0.03,
            markup_pct: 2.0,
            daily_cap_usd: 0.50,
            monthly_cap_usd: 5.00,
        }
    }

    fn context() -> BillingContext {
        context_with(cfg())
    }

    fn context_with(wrapper_config: BillingWrapperConfig) -> BillingContext {
        BillingContext {
            service: BillingService::open_in_memory().unwrap(),
            wrapper_config,
            pricing: PricingTable::builtin(),
        }
    }

    fn priced_result(
        text: &str,
        cost_usd: f64,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) -> TextGenerationResult {
        let mut result = TextGenerationResult::text_only(text.to_owned());
        let usage = result.usage.get_or_insert_with(Default::default);
        usage.cost_usd = Some(cost_usd);
        usage.prompt_tokens = Some(prompt_tokens);
        usage.completion_tokens = Some(completion_tokens);
        result
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn allow_path_creates_reserve_and_settles() {
        let ctx = context();
        ctx.service
            .topup(ROOM, 1.00, Some(USER), json!({}))
            .await
            .unwrap();

        let decision = pre_check_and_reserve(&ctx, ROOM, USER, EVENT).await;
        let corr = match decision {
            BillingDecision::Allow { correlation_id } => correlation_id,
            other => panic!("expected Allow, got {other:?}"),
        };

        // Reserve is in the ledger.
        let bal = ctx.service.compute_balance(ROOM).await.unwrap();
        assert!((bal - 0.97).abs() < 1e-9, "after reserve: {bal}");

        let result = priced_result("ok", 0.005, 100, 50);
        let summary = settle_charge(&ctx, corr, 2.0, MODEL, &result)
            .await
            .unwrap();
        assert!((summary.charged_usd - 0.010).abs() < 1e-12, "{summary:?}");
        assert!(!summary.used_fallback_pricing);

        // Final balance: 1.00 (topup) - 0.03 (reserve) + 0.03 (release) - 0.010 (charge) = 0.99
        let bal = ctx.service.compute_balance(ROOM).await.unwrap();
        assert!((bal - 0.99).abs() < 1e-9, "final balance: {bal}");

        // 4 ledger rows: topup, reserve, charge, release.
        let events = ctx.service.recent_events(ROOM, 10).await.unwrap();
        assert_eq!(events.len(), 4);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn insufficient_balance_returns_decision_without_reserve() {
        let ctx = context();
        // No topup: balance is 0.
        let decision = pre_check_and_reserve(&ctx, ROOM, USER, EVENT).await;
        match decision {
            BillingDecision::InsufficientBalance {
                current_balance_usd,
                reserve_amount_usd,
            } => {
                assert!(current_balance_usd.abs() < 1e-9);
                assert!((reserve_amount_usd - 0.03).abs() < 1e-12);
            }
            other => panic!("expected InsufficientBalance, got {other:?}"),
        }
        // No reserve row.
        assert_eq!(ctx.service.recent_events(ROOM, 10).await.unwrap().len(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn daily_cap_hit_returns_decision_without_reserve() {
        let ctx = context();
        ctx.service
            .topup(ROOM, 100.0, Some(USER), json!({}))
            .await
            .unwrap();
        // Drive daily-spent past the cap.
        let prev = Uuid::now_v7();
        ctx.service
            .reserve(ROOM, prev, 0.03, Some(USER), Some(EVENT))
            .await
            .unwrap();
        ctx.service.charge(prev, 0.49, json!({})).await.unwrap();
        ctx.service.release(prev).await.unwrap();
        let decision = pre_check_and_reserve(&ctx, ROOM, USER, EVENT).await;
        match decision {
            BillingDecision::CapHit {
                period: CapPeriod::Daily,
                cap_usd,
                ..
            } => {
                assert_eq!(cap_usd, 0.50);
            }
            other => panic!("expected CapHit(Daily), got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn settle_falls_back_to_estimated_tokens() {
        let ctx = context();
        ctx.service
            .topup(ROOM, 1.0, Some(USER), json!({}))
            .await
            .unwrap();
        let decision = pre_check_and_reserve(&ctx, ROOM, USER, EVENT).await;
        let corr = match decision {
            BillingDecision::Allow { correlation_id } => correlation_id,
            other => panic!("expected Allow, got {other:?}"),
        };
        // Provider returned no usage info: the wrapper estimates from text length.
        let result = TextGenerationResult::text_only("x".repeat(400)); // ~100 tokens
        let summary = settle_charge(&ctx, corr, 2.0, MODEL, &result)
            .await
            .unwrap();
        assert!(summary.used_fallback_pricing);
        // 100 completion * $0.0000012 = $0.00012 ; markup x2 = $0.00024
        assert!((summary.charged_usd - 0.00024).abs() < 1e-7, "{summary:?}");

        let rows = ctx.service.recent_events(ROOM, 10).await.unwrap();
        let charge = rows
            .iter()
            .find(|e| e.event_type == BillingEventType::Charge)
            .expect("charge row");
        assert_eq!(charge.meta["estimated_tokens"], json!(true));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn settle_unknown_model_charges_zero_and_releases() {
        let ctx = context();
        ctx.service
            .topup(ROOM, 1.0, Some(USER), json!({}))
            .await
            .unwrap();
        let decision = pre_check_and_reserve(&ctx, ROOM, USER, EVENT).await;
        let corr = match decision {
            BillingDecision::Allow { correlation_id } => correlation_id,
            other => panic!("expected Allow, got {other:?}"),
        };
        let result = TextGenerationResult::text_only("hello".to_owned());
        let summary = settle_charge(&ctx, corr, 2.0, "acme/unpriced", &result)
            .await
            .unwrap();
        assert_eq!(summary.charged_usd, 0.0);
        assert!(summary.used_fallback_pricing);

        let bal = ctx.service.compute_balance(ROOM).await.unwrap();
        assert!(
            (bal - 1.0).abs() < 1e-9,
            "reserve released, nothing charged: {bal}"
        );
        let rows = ctx.service.recent_events(ROOM, 10).await.unwrap();
        let charge = rows
            .iter()
            .find(|e| e.event_type == BillingEventType::Charge)
            .expect("charge row");
        assert_eq!(charge.meta["unknown_model_audit"], json!(true));
    }

    #[test]
    fn effective_markup_pct_uses_room_override_when_present() {
        assert!((effective_markup_pct(Some(1.5), 2.0) - 1.5).abs() < 1e-12);
    }

    #[test]
    fn effective_markup_pct_falls_back_when_override_invalid() {
        assert!((effective_markup_pct(None, 2.0) - 2.0).abs() < 1e-12);
        assert!((effective_markup_pct(Some(0.0), 2.0) - 2.0).abs() < 1e-12);
        assert!((effective_markup_pct(Some(-1.0), 2.0) - 2.0).abs() < 1e-12);
        assert!((effective_markup_pct(Some(f64::NAN), 2.0) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("a".repeat(400).as_str()), 100);
    }

    // End-to-end coverage of the chain that
    // `chat_completion::handle_stage_text_generation` runs around the LLM
    // call. Each test mirrors a real production path with a closure standing
    // in for the agent call.

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_billed_call_without_billing_invokes_llm_directly() {
        let outcome = run_billed_call(None, ROOM, USER, EVENT, None, MODEL, || async {
            Ok(priced_result("ok", 0.005, 100, 50))
        })
        .await;

        match outcome {
            BilledCallOutcome::Unbilled { text_generation } => {
                assert_eq!(text_generation.text, "ok");
            }
            other => panic!("expected Unbilled, got {other:?}"),
        }

        let outcome = run_billed_call(None, ROOM, USER, EVENT, None, MODEL, || async {
            Err(anyhow::anyhow!("provider blew up"))
        })
        .await;

        match outcome {
            BilledCallOutcome::LlmError {
                correlation_id,
                error,
            } => {
                assert!(correlation_id.is_none());
                assert!(error.to_string().contains("blew up"));
            }
            other => panic!("expected LlmError, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_billed_call_happy_path_writes_topup_reserve_charge_release_in_order() {
        let ctx = context();
        ctx.service
            .topup(ROOM, 1.0, Some(USER), json!({}))
            .await
            .unwrap();

        let outcome = run_billed_call(Some(&ctx), ROOM, USER, EVENT, None, MODEL, || async {
            Ok(priced_result("ok", 0.005, 100, 50))
        })
        .await;

        match outcome {
            BilledCallOutcome::Success {
                text_generation,
                summary,
                ..
            } => {
                assert_eq!(text_generation.text, "ok");
                assert!((summary.charged_usd - 0.010).abs() < 1e-12);
                assert!(!summary.used_fallback_pricing);
            }
            other => panic!("expected Success, got {other:?}"),
        }

        // Ledger order check (created_at ASC). recent_events is DESC by
        // created_at, so reverse to get insertion order.
        let mut rows = ctx.service.recent_events(ROOM, 10).await.unwrap();
        rows.reverse();
        let types: Vec<BillingEventType> = rows.iter().map(|e| e.event_type).collect();
        assert_eq!(
            types,
            vec![
                BillingEventType::Topup,
                BillingEventType::Reserve,
                BillingEventType::Charge,
                BillingEventType::Release,
            ],
            "chain must produce the four ledger rows in this order: {types:?}"
        );

        // Final balance: 1.00 (topup) - 0.03 (reserve) + 0.03 (release) - 0.010 (charge) = 0.99
        let bal = ctx.service.compute_balance(ROOM).await.unwrap();
        assert!((bal - 0.99).abs() < 1e-9, "final balance: {bal}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_billed_call_applies_room_markup_override() {
        let ctx = context();
        ctx.service
            .topup(ROOM, 1.0, Some(USER), json!({}))
            .await
            .unwrap();

        let outcome = run_billed_call(Some(&ctx), ROOM, USER, EVENT, Some(1.5), MODEL, || async {
            Ok(priced_result("ok", 0.010, 100, 50))
        })
        .await;

        match outcome {
            BilledCallOutcome::Success { summary, .. } => {
                assert!((summary.charged_usd - 0.015).abs() < 1e-12, "{summary:?}");
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_billed_call_llm_error_path_leaves_reserve_alone() {
        // On LLM Err, the reserve stays in the ledger as a zombie: no charge,
        // no release. A refactor that forgets to skip settle on Err regresses here.
        let ctx = context();
        ctx.service
            .topup(ROOM, 1.0, Some(USER), json!({}))
            .await
            .unwrap();

        let outcome = run_billed_call(Some(&ctx), ROOM, USER, EVENT, None, MODEL, || async {
            Err(anyhow::anyhow!("provider blew up"))
        })
        .await;

        match outcome {
            BilledCallOutcome::LlmError {
                correlation_id,
                error,
            } => {
                assert!(correlation_id.is_some_and(|id| id != Uuid::nil()));
                assert!(error.to_string().contains("blew up"));
            }
            other => panic!("expected LlmError, got {other:?}"),
        }

        let rows = ctx.service.recent_events(ROOM, 10).await.unwrap();
        assert_eq!(rows.len(), 2, "expected only topup + reserve, got {rows:?}");
        let types: Vec<BillingEventType> = rows.iter().map(|e| e.event_type).collect();
        assert!(
            types.contains(&BillingEventType::Topup) && types.contains(&BillingEventType::Reserve),
            "rows: {types:?}"
        );
        assert!(
            !types.contains(&BillingEventType::Charge),
            "must NOT charge on LLM error"
        );
        assert!(
            !types.contains(&BillingEventType::Release),
            "must NOT release on LLM error: the reserve is the zombie"
        );
        // Zombie surfaces in `list_zombies`.
        let zombies = ctx.service.list_zombies(0).await.unwrap();
        assert_eq!(zombies.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_billed_call_daily_cap_hit_does_not_invoke_llm() {
        let ctx = context();
        ctx.service
            .topup(ROOM, 100.0, Some(USER), json!({}))
            .await
            .unwrap();
        // Drive daily-spent past the $0.50 cap.
        let prev = Uuid::now_v7();
        ctx.service
            .reserve(ROOM, prev, 0.03, Some(USER), Some(EVENT))
            .await
            .unwrap();
        ctx.service.charge(prev, 0.49, json!({})).await.unwrap();
        ctx.service.release(prev).await.unwrap();

        let llm_calls = AtomicUsize::new(0);
        let outcome = run_billed_call(Some(&ctx), ROOM, USER, EVENT, None, MODEL, || async {
            llm_calls.fetch_add(1, Ordering::SeqCst);
            Ok(TextGenerationResult::text_only("should never run".into()))
        })
        .await;

        match outcome {
            BilledCallOutcome::CapHit {
                period: CapPeriod::Daily,
                cap_usd,
                ..
            } => {
                assert_eq!(cap_usd, 0.50);
            }
            other => panic!("expected CapHit(Daily), got {other:?}"),
        }
        assert_eq!(
            llm_calls.load(Ordering::SeqCst),
            0,
            "LLM closure must NOT run when the daily cap is hit"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_billed_call_monthly_cap_hit_does_not_invoke_llm() {
        // pre_check checks daily first, then monthly. To isolate the monthly
        // path, widen the daily cap so only the monthly one trips.
        let mut wide_cfg = cfg();
        wide_cfg.daily_cap_usd = 100.0;
        let ctx = context_with(wide_cfg);
        ctx.service
            .topup(ROOM, 100.0, Some(USER), json!({}))
            .await
            .unwrap();
        for i in 0..11 {
            let corr = Uuid::now_v7();
            ctx.service
                .reserve(ROOM, corr, 0.50, Some(USER), Some(EVENT))
                .await
                .unwrap();
            ctx.service
                .charge(corr, 0.46, json!({"i": i}))
                .await
                .unwrap();
            ctx.service.release(corr).await.unwrap();
        }

        let llm_calls = AtomicUsize::new(0);
        let outcome = run_billed_call(Some(&ctx), ROOM, USER, EVENT, None, MODEL, || async {
            llm_calls.fetch_add(1, Ordering::SeqCst);
            Ok(TextGenerationResult::text_only("should never run".into()))
        })
        .await;

        match outcome {
            BilledCallOutcome::CapHit {
                period: CapPeriod::Monthly,
                cap_usd,
                ..
            } => {
                assert_eq!(cap_usd, 5.00);
            }
            other => panic!("expected CapHit(Monthly), got {other:?}"),
        }
        assert_eq!(
            llm_calls.load(Ordering::SeqCst),
            0,
            "LLM closure must NOT run when the monthly cap is hit"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_billed_call_settle_failed_returns_text_and_error() {
        // The LLM ran successfully but the writeback collided (here: a
        // duplicate charge). The user still gets their answer; the reserve
        // becomes a zombie for manual reconciliation.
        let ctx = context();
        ctx.service
            .topup(ROOM, 1.0, Some(USER), json!({}))
            .await
            .unwrap();

        // Drive a duplicate charge by force-charging *inside* the LLM closure
        // (BillingService is Clone via Arc). By the time `settle_charge`'s
        // `charge(...)` runs, the same correlation_id already has a charge row.
        let svc_handle = ctx.service.clone();
        let outcome = run_billed_call(Some(&ctx), ROOM, USER, EVENT, None, MODEL, || {
            let svc_handle = svc_handle.clone();
            async move {
                // Find the just-reserved correlation_id.
                let zombies = svc_handle.list_zombies(0).await.unwrap();
                assert_eq!(zombies.len(), 1, "expected exactly one fresh reserve");
                let corr = zombies[0]
                    .event
                    .correlation_id
                    .expect("reserve event must carry a correlation_id");
                // Pre-charge so settle's `charge` collides.
                svc_handle
                    .charge(corr, 0.001, json!({"sentinel": "pre-charged"}))
                    .await
                    .unwrap();
                Ok(priced_result("user-facing reply", 0.005, 50, 50))
            }
        })
        .await;

        match outcome {
            BilledCallOutcome::SettleFailed {
                text_generation,
                correlation_id,
                error,
            } => {
                // Caller still sends the user the answer; that's the whole
                // point of the variant.
                assert_eq!(text_generation.text, "user-facing reply");
                assert_ne!(correlation_id, Uuid::nil());
                assert!(
                    matches!(error, BillingError::DuplicateCharge(_)),
                    "expected DuplicateCharge, got {error:?}"
                );
            }
            other => panic!("expected SettleFailed, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_billed_call_insufficient_balance_does_not_invoke_llm() {
        // No topup: balance starts at 0.
        let ctx = context();
        let llm_calls = AtomicUsize::new(0);

        let outcome = run_billed_call(Some(&ctx), ROOM, USER, EVENT, None, MODEL, || async {
            llm_calls.fetch_add(1, Ordering::SeqCst);
            Ok(TextGenerationResult::text_only("should never run".into()))
        })
        .await;

        match outcome {
            BilledCallOutcome::InsufficientBalance {
                current_balance_usd,
                reserve_amount_usd,
            } => {
                assert!(current_balance_usd.abs() < 1e-9);
                assert!((reserve_amount_usd - 0.03).abs() < 1e-12);
            }
            other => panic!("expected InsufficientBalance, got {other:?}"),
        }

        assert_eq!(
            llm_calls.load(Ordering::SeqCst),
            0,
            "LLM closure must NOT run when the pre-check rejects"
        );
        // No ledger rows at all.
        assert_eq!(ctx.service.recent_events(ROOM, 10).await.unwrap().len(), 0);
    }
}
