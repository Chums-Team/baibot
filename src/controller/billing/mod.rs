//! Chat commands of the billing feature: `balance`, `stats` and the `billing ...`
//! administration family.
//!
//! The parser (`determination`) and the handlers are pure: they take plain values and
//! return either a `BillingControllerType` or the markdown to send. The Matrix-side glue
//! lives in `dispatching`, which keeps the command logic testable without a running bot.

mod controller_type;
mod determination;
mod dispatching;
mod handlers;

pub use controller_type::{BillingCommandAccess, BillingControllerType};
pub use determination::{COMMAND_HEADS, determine_controller};
pub use dispatching::dispatch_controller;

#[cfg(test)]
mod tests;
