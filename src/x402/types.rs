//! Wire types of the sidecar → bot direction. Mirrors `InternalSettledNotify` in
//! `x402-sidecar/src/x402_sidecar/models.py`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Body of `POST /internal/x402-settled`, the only message the sidecar pushes to the bot.
/// Authenticated by the `X-Internal-Signature` HMAC header, see `webhook`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InternalSettledNotify {
    pub payment_id: Uuid,
    pub room_id: String,
    pub user_mxid: String,
    pub amount_usd: f64,
    pub tx_hash: String,
    pub block_number: u64,
    pub settled_at: DateTime<Utc>,
    /// Present when the original payment request carried one. The bot does not act on it
    /// (a top-up is room-scoped, not call-scoped); it lets operators trace a single LLM call
    /// across services.
    #[serde(default)]
    pub correlation_id: Option<Uuid>,
}
