pub fn temporarily_unavailable() -> &'static str {
    "Billing is temporarily unavailable. Please try again later."
}

pub fn insufficient_balance(current_balance_usd: f64, reserve_amount_usd: f64) -> String {
    format!(
        "Agent balance in this room: ${current_balance_usd:.4}. At least ${reserve_amount_usd:.2} is needed for the next reply. Top up the room balance to continue.",
    )
}

/// Sent together with a `cc.chums.x402_request` event.
pub fn insufficient_balance_with_widget(
    current_balance_usd: f64,
    reserve_amount_usd: f64,
) -> String {
    format!(
        "Agent balance in this room: ${current_balance_usd:.4}. At least ${reserve_amount_usd:.2} is needed for the next reply. Top up below.",
    )
}

/// Sent when the payment sidecar could not be reached, so no widget accompanies the text.
pub fn insufficient_balance_with_topup_command(
    current_balance_usd: f64,
    reserve_amount_usd: f64,
    command_prefix: &str,
    topup_amount_usd: f64,
) -> String {
    format!(
        "Agent balance in this room: ${current_balance_usd:.4}. At least ${reserve_amount_usd:.2} is needed for the next reply. Top up with `{command_prefix} topup {topup_amount_usd:.2}`.",
    )
}

pub fn topup_confirmed(amount_usd: f64, new_balance_usd: f64, tx_hash: &str) -> String {
    format!(
        "✓ Topped up ${amount_usd:.4}. Agent balance in this room: ${new_balance_usd:.4}.\ntx: {tx_hash}"
    )
}

pub fn topup_not_configured() -> &'static str {
    "Top-ups from chat are not available on this bot: the x402 payment sidecar is not configured."
}

pub fn topup_amount_out_of_range(min_topup_usd: f64, max_topup_usd: f64) -> String {
    format!("The amount must be between ${min_topup_usd:.2} and ${max_topup_usd:.2}.")
}

pub fn topup_request_failed(error: &str) -> String {
    format!("Could not initiate the top-up: {error}. Please try again later.")
}

pub fn cap_hit(period: &str, cap_usd: f64, resumes_at: &str) -> String {
    format!("Service paused ({period} cap ${cap_usd:.2} reached). Resumes at {resumes_at}.")
}

pub fn access_denied(command: &str) -> String {
    format!(
        "Access denied: the `{command}` command is only available to billing administrators (see `billing.admin_mxids` in the configuration)."
    )
}

pub fn invalid_command(command: &str, reason: &str) -> String {
    format!("Invalid `{command}` command: {reason}.")
}

pub fn command_failed(command: &str, error: &str) -> String {
    format!("The `{command}` command failed: {error}")
}
