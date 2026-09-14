pub fn temporarily_unavailable() -> &'static str {
    "Billing is temporarily unavailable. Please try again later."
}

pub fn insufficient_balance(current_balance_usd: f64, reserve_amount_usd: f64) -> String {
    format!(
        "Agent balance in this room: ${current_balance_usd:.4}. At least ${reserve_amount_usd:.2} is needed for the next reply. Top up the room balance to continue.",
    )
}

pub fn cap_hit(period: &str, cap_usd: f64, resumes_at: &str) -> String {
    format!("Service paused ({period} cap ${cap_usd:.2} reached). Resumes at {resumes_at}.")
}
