//! Public types for the billing ledger.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single immutable entry in the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingEvent {
    pub event_id: Uuid,
    pub room_id: String,
    #[serde(rename = "type")]
    pub event_type: BillingEventType,
    pub amount_usd: f64,
    /// Correlates `reserve` <-> `charge` <-> `release` of one LLM call.
    /// `None` for `topup` and `refund_manual`.
    pub correlation_id: Option<Uuid>,
    pub user_mxid: Option<String>,
    pub matrix_event_id: Option<String>,
    pub meta: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingEventType {
    /// Pre-LLM hold of a fixed reserve (e.g. $0.03). Negative amount.
    Reserve,
    /// Post-LLM actual cost × markup. Negative amount.
    Charge,
    /// Releases the matching reserve. Positive amount = reserve_amount.
    Release,
    /// User pays in (via x402). Positive amount.
    Topup,
    /// Admin-issued refund of a top-up (off-chain dispute resolution).
    /// Positive amount.
    RefundManual,
}

impl BillingEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            BillingEventType::Reserve => "reserve",
            BillingEventType::Charge => "charge",
            BillingEventType::Release => "release",
            BillingEventType::Topup => "topup",
            BillingEventType::RefundManual => "refund_manual",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reserve" => Some(Self::Reserve),
            "charge" => Some(Self::Charge),
            "release" => Some(Self::Release),
            "topup" => Some(Self::Topup),
            "refund_manual" => Some(Self::RefundManual),
            _ => None,
        }
    }
}

/// Window for `compute_period_spent()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    /// Calendar day in UTC.
    Day,
    /// Calendar month in UTC.
    Month,
}

/// A reserve that has no matching charge/release older than the threshold.
/// Surfaces from `BillingService::list_zombies()`.
#[derive(Debug, Clone)]
pub struct ZombieReserve {
    pub event: BillingEvent,
    pub age_seconds: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum BillingError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("background task panicked or was cancelled: {0}")]
    Join(String),

    /// `reserve()` called twice for the same correlation_id.
    #[error("duplicate reserve for correlation {0}")]
    DuplicateReserve(Uuid),

    /// `charge()`/`release()` called without a matching reserve.
    #[error("no reserve found for correlation {0}")]
    ReserveNotFound(Uuid),

    /// `charge()` called more than once for the same correlation.
    #[error("charge already exists for correlation {0}")]
    DuplicateCharge(Uuid),

    /// `release()` called more than once for the same correlation.
    #[error("release already exists for correlation {0}")]
    DuplicateRelease(Uuid),

    /// `manual_release` referenced a reserve that doesn't exist.
    #[error("reserve event {0} not found")]
    ReserveEventNotFound(Uuid),

    /// `topup()` raced another webhook for the same `payment_id` and
    /// the partial UNIQUE index in `002_payment_id_unique.sql` rejected
    /// the second INSERT. Webhook handler maps this to HTTP 409 (same
    /// response as the pre-INSERT idempotency check).
    #[error("topup already credited for payment_id {0}")]
    DuplicatePaymentId(Uuid),
}

impl From<tokio::task::JoinError> for BillingError {
    fn from(e: tokio::task::JoinError) -> Self {
        BillingError::Join(e.to_string())
    }
}
