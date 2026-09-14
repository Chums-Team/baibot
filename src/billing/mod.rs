//! Append-only billing ledger.
//!
//! Backed by SQLite (`rusqlite` with `bundled` feature; no system SQLite
//! needed). Public API is async — internally each call hops to a blocking
//! thread via `tokio::task::spawn_blocking`, and access to the connection
//! is serialized by a `Mutex` (SQLite is single-writer anyway).

pub mod ledger;
pub mod pricing;
pub mod types;
pub mod wrapper;

#[cfg(test)]
mod tests;

pub use ledger::BillingService;
pub use pricing::{ModelPrice, PRICING_TABLE_REVISION, PricingTable};
pub use types::{BillingError, BillingEvent, BillingEventType, Period, ZombieReserve};
pub use wrapper::{
    BillingWrapperConfig, ChargeBreakdown, PreCheckOutcome, compute_charge, new_correlation_id,
    pre_check,
};

/// The billing feature as wired into a running bot: the ledger plus the policy applied to
/// every LLM call. Built once at startup from the `billing` configuration section and absent
/// when that section is absent.
#[derive(Clone)]
pub struct BillingContext {
    pub service: BillingService,
    pub wrapper_config: BillingWrapperConfig,
    /// Fallback prices for calls whose cost the provider does not report.
    pub pricing: PricingTable,
}
