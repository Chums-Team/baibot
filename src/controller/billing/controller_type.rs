use mxlink::matrix_sdk::ruma::UserId;
use uuid::Uuid;

use crate::entity::cfg::ConfigBilling;

/// What the sender may do with the billing commands. Determined once per message from the
/// `billing` configuration section and passed to the router explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingCommandAccess {
    /// Billing is not configured. The commands do not exist and the text is routed as if
    /// the feature were absent.
    Disabled,
    /// Billing is configured. The sender may run the public commands only.
    User,
    /// Billing is configured and the sender is listed in `billing.admin_mxids`.
    Admin,
}

impl BillingCommandAccess {
    pub fn determine(config: Option<&ConfigBilling>, sender_id: &UserId) -> Self {
        match config {
            None => Self::Disabled,
            Some(config) if config.is_admin(sender_id.as_str()) => Self::Admin,
            Some(_) => Self::User,
        }
    }

    pub fn is_admin(self) -> bool {
        matches!(self, Self::Admin)
    }
}

/// A parsed billing command. Variants carry only what the parser extracted from the text;
/// `dispatching` turns them into a reply.
#[derive(Debug, Clone, PartialEq)]
pub enum BillingControllerType {
    /// `billing` / `billing help`. Also shown after a parse error.
    Help,
    /// `balance`: the balance of the current room and its last few ledger entries.
    Balance,
    /// `stats day` (administrators): bot-wide spending today against the daily cap.
    StatsDay,
    /// `stats month` (administrators): bot-wide spending this month against the monthly cap.
    StatsMonth,
    /// `billing zombies [<minutes>]` (administrators): reserves without a charge or release.
    Zombies { older_than_minutes: u32 },
    /// `billing manual-release <event_id> "<reason>"` (administrators).
    ManualRelease {
        reserve_event_id: Uuid,
        reason: String,
    },
    /// `billing manual-refund <room_id> <amount_usd> "<reason>"` (administrators).
    ManualRefund {
        room_id: String,
        amount_usd: f64,
        reason: String,
    },
    /// An administrator-only command sent by someone else. Nothing is looked up.
    AccessDenied { command: &'static str },
    /// The command was recognised but its arguments were not.
    ParseError { command: String, reason: String },
}
