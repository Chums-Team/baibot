// rustc 1.94+ trips a query-depth overflow when computing async layouts in
// the matrix-sdk timeline future graph. matrix-rust-sdk PR #6489 raises the
// limit, but `recursion_limit` is per-crate and applies to the crate currently
// being compiled — so the consumer has to repeat it.
#![recursion_limit = "256"]

// Translations of the bot's replies (`locales/<code>.yml`) are embedded at compile time;
// `en` is the last resort when a key is missing in a locale. See `src/i18n`.
rust_i18n::i18n!("locales", fallback = "en");

mod agent;
pub mod billing;
mod bot;
mod controller;
mod conversation;
mod entity;
mod i18n;
pub mod matrix;
mod strings;
#[cfg(feature = "tron-login")]
pub mod tron_login;
mod utils;
mod x402;

pub use bot::{Bot, load_config};
pub use entity::cfg::Config;
