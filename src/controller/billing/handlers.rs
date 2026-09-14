//! Pure handlers for the billing chat commands. Each returns the markdown the bot sends to
//! the room. The only side effects are the explicit ledger writes of the `manual-*` commands.

use uuid::Uuid;

use crate::billing::{BillingError, BillingService, Period};

// ===== `balance` =====

pub async fn balance(
    billing: &BillingService,
    room_id: &str,
    show_recent: usize,
) -> Result<String, BillingError> {
    let balance = billing.compute_balance(room_id).await?;

    let mut out = format!("**Balance:** ${balance:.2}\n");

    if show_recent > 0 {
        let events = billing.recent_events(room_id, show_recent).await?;
        if !events.is_empty() {
            out.push_str("\n**Recent transactions:**\n\n");
            out.push_str("| When (UTC) | Type | Amount |\n");
            out.push_str("|---|---|---|\n");
            for event in events {
                let sign = if event.amount_usd >= 0.0 { "+" } else { "" };
                out.push_str(&format!(
                    "| {} | {} | {sign}${:.4} |\n",
                    event.created_at.format("%Y-%m-%d %H:%M"),
                    event.event_type.as_str(),
                    event.amount_usd,
                ));
            }
        }
    }

    Ok(out)
}

// ===== `stats day` / `stats month` (administrators) =====

pub async fn stats(
    billing: &BillingService,
    period: Period,
    cap_usd: f64,
) -> Result<String, BillingError> {
    let spent = billing.compute_period_spent(period).await?;

    let percent = if cap_usd > 0.0 {
        100.0 * spent / cap_usd
    } else {
        0.0
    };

    let label = match period {
        Period::Day => "Today (UTC)",
        Period::Month => "This month (UTC)",
    };

    Ok(format!(
        "**{label}** spend: ${spent:.4} / ${cap_usd:.2} ({percent:.1}%)"
    ))
}

// ===== `billing zombies [<minutes>]` (administrators) =====

pub async fn list_zombies(
    billing: &BillingService,
    older_than_minutes: u32,
    command_prefix: &str,
) -> Result<String, BillingError> {
    let zombies = billing.list_zombies(older_than_minutes).await?;

    if zombies.is_empty() {
        return Ok(format!(
            "No zombie reserves older than {older_than_minutes} min. ✓"
        ));
    }

    let mut out = format!(
        "**{} zombie reserve(s)** older than {older_than_minutes} min:\n\n",
        zombies.len()
    );
    out.push_str("| event_id | room | correlation_id | amount | age (s) |\n");
    out.push_str("|---|---|---|---|---|\n");
    for zombie in zombies {
        out.push_str(&format!(
            "| `{}` | `{}` | `{}` | ${:.4} | {} |\n",
            zombie.event.event_id,
            zombie.event.room_id,
            zombie
                .event
                .correlation_id
                .map(|uuid| uuid.to_string())
                .unwrap_or_else(|| "-".to_owned()),
            zombie.event.amount_usd,
            zombie.age_seconds,
        ));
    }
    out.push_str(&format!(
        "\nResolve with `{command_prefix} billing manual-release <event_id> \"<reason>\"`."
    ));

    Ok(out)
}

// ===== `billing manual-release <event_id> "<reason>"` (administrators) =====

pub async fn manual_release(
    billing: &BillingService,
    reserve_event_id: Uuid,
    admin_mxid: &str,
    reason: &str,
) -> Result<String, BillingError> {
    let release = billing
        .manual_release(reserve_event_id, admin_mxid, reason)
        .await?;

    Ok(format!(
        "Released zombie reserve. Compensating event `{}` (room `{}`) credited ${:.4}. Reason: _{reason}_",
        release.event_id, release.room_id, release.amount_usd,
    ))
}

// ===== `billing manual-refund <room_id> <amount> "<reason>"` (administrators) =====

pub async fn manual_refund(
    billing: &BillingService,
    room_id: &str,
    amount_usd: f64,
    admin_mxid: &str,
    reason: &str,
) -> Result<String, BillingError> {
    let event = billing
        .manual_refund(room_id, amount_usd, admin_mxid, reason)
        .await?;

    Ok(format!(
        "Refunded ${:.4} to room `{}`. Compensating event `{}`. Reason: _{reason}_",
        event.amount_usd, event.room_id, event.event_id,
    ))
}

// ===== `topup` =====

/// The text sent together with a `cc.chums.x402_request` event, for clients that do not
/// render the payment widget.
pub fn topup_invoice(amount_required_usd: f64) -> String {
    format!(
        "To top up the agent's balance in this room, pay **${amount_required_usd:.2}** in USDT (TRC-20) through the payment widget. \
         If no widget appears, your client does not support x402 payments yet; use the Chums web client."
    )
}

// ===== `billing help` =====

/// `topup_available` is whether the `x402` section is configured; the `topup` command is
/// listed only then.
pub fn help(command_prefix: &str, is_admin: bool, topup_available: bool) -> String {
    let mut out = format!(
        "- `{command_prefix} balance` — show the agent's balance in this room and recent transactions.\n"
    );

    if topup_available {
        out.push_str(&format!(
            "- `{command_prefix} topup [<amount_usd>]` — top up the balance of this room with a USDT (TRC-20) payment.\n"
        ));
    }

    if is_admin {
        out.push_str("\n**Administration commands**\n\n");
        out.push_str(&format!(
            "- `{command_prefix} stats day | month` — bot-wide LLM spend against the cap.\n"
        ));
        out.push_str(&format!(
            "- `{command_prefix} billing zombies [<minutes>]` — list reserves without a charge or release (default 10 min).\n"
        ));
        out.push_str(&format!(
            "- `{command_prefix} billing manual-release <event_id> \"<reason>\"` — credit a stuck reserve back.\n"
        ));
        out.push_str(&format!(
            "- `{command_prefix} billing manual-refund <room_id> <amount> \"<reason>\"` — refund a top-up after an off-chain return.\n"
        ));
    }

    out
}
