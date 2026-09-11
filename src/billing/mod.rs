//! Append-only billing ledger.
//!
//! Backed by SQLite (`rusqlite` with `bundled` feature; no system SQLite
//! needed). Public API is async — internally each call hops to a blocking
//! thread via `tokio::task::spawn_blocking`, and access to the connection
//! is serialized by a `Mutex` (SQLite is single-writer anyway).

// Nothing in the crate consumes the ledger yet; the chat-completion controller is wired to it
// in a follow-up commit, at which point this allow goes away.
#![allow(dead_code, unused_imports)]

pub mod ledger;
pub mod pricing;
pub mod types;
pub mod wrapper;

#[cfg(test)]
mod tests;

pub use ledger::BillingService;
pub use pricing::{ModelPrice, PRICING_TABLE_REVISION, compute_fallback, lookup};
pub use types::{BillingError, BillingEvent, BillingEventType, Period, ZombieReserve};
pub use wrapper::{
    BillingWrapperConfig, ChargeBreakdown, PreCheckOutcome, compute_charge, new_correlation_id,
    pre_check,
};
