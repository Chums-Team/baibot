//! Top-ups of the billing ledger through the x402 payment sidecar.
//!
//! The bot does not speak the x402 wire format itself; that is the job of the sidecar in
//! `x402-sidecar/`. The bot only needs two things:
//!
//! - **Outbound**: `client` asks the sidecar (`POST /payment-request`) for a payment request
//!   and shapes the answer into a `cc.chums.x402_request` event, which the Chums client
//!   renders as a payment widget. The same client forwards the signature the payer sends back
//!   as a `cc.chums.x402_submit` event (`POST /payment-request/submit`, after a
//!   `GET /status/{payment_id}` check); see `src/bot/x402.rs`.
//! - **Inbound**: `webhook` receives the sidecar's HMAC-signed `POST /internal/x402-settled`
//!   after a payment settles, credits the room in the ledger and hands the bot an
//!   announcement to post into the room. `server` binds the endpoint.
//!
//! Enabled by the `x402` configuration section (see `docs/configuration/x402.md`).

mod client;
mod server;
mod types;
mod webhook;

pub use client::{PaymentSubmit, X402Client};
pub use server::start_webhook_server;
pub use webhook::{TopupAnnouncement, WebhookState};
