//! Replies of the billing feature.
//!
//! Every function takes the locale of the addressed user (see `crate::i18n`) and renders a key
//! from `locales/<locale>.yml`. Amounts are formatted here, so every locale shows the same
//! figures: balances and credited top-ups with 4 decimals, thresholds and limits with 2.

use rust_i18n::t;

use crate::controller::chat_completion::billing_glue::CapPeriod;

pub fn temporarily_unavailable(locale: &str) -> String {
    t!("billing.temporarily_unavailable", locale = locale).into_owned()
}

/// Sent when the x402 integration is not configured: no way to top up from the chat.
pub fn insufficient_balance(
    locale: &str,
    current_balance_usd: f64,
    reserve_amount_usd: f64,
) -> String {
    t!(
        "billing.insufficient_balance",
        locale = locale,
        balance = format!("{current_balance_usd:.4}"),
        reserve = format!("{reserve_amount_usd:.2}"),
    )
    .into_owned()
}

/// Sent together with a `cc.chums.x402_request` event.
pub fn insufficient_balance_with_widget(
    locale: &str,
    current_balance_usd: f64,
    reserve_amount_usd: f64,
) -> String {
    t!(
        "billing.insufficient_balance_with_widget",
        locale = locale,
        balance = format!("{current_balance_usd:.4}"),
        reserve = format!("{reserve_amount_usd:.2}"),
    )
    .into_owned()
}

/// Sent when the payment sidecar could not be reached, so no widget accompanies the text.
pub fn insufficient_balance_with_topup_command(
    locale: &str,
    current_balance_usd: f64,
    reserve_amount_usd: f64,
    command_prefix: &str,
    topup_amount_usd: f64,
) -> String {
    t!(
        "billing.insufficient_balance_with_topup_command",
        locale = locale,
        balance = format!("{current_balance_usd:.4}"),
        reserve = format!("{reserve_amount_usd:.2}"),
        prefix = command_prefix,
        amount = format!("{topup_amount_usd:.2}"),
    )
    .into_owned()
}

pub fn cap_hit(locale: &str, period: CapPeriod, cap_usd: f64, resumes_at: &str) -> String {
    let cap = format!("{cap_usd:.2}");
    match period {
        CapPeriod::Daily => t!(
            "billing.cap_hit.daily",
            locale = locale,
            cap = cap,
            resumes_at = resumes_at
        ),
        CapPeriod::Monthly => t!(
            "billing.cap_hit.monthly",
            locale = locale,
            cap = cap,
            resumes_at = resumes_at
        ),
    }
    .into_owned()
}

pub fn topup_confirmed(
    locale: &str,
    amount_usd: f64,
    new_balance_usd: f64,
    tx_hash: &str,
) -> String {
    t!(
        "billing.topup_confirmed",
        locale = locale,
        amount = format!("{amount_usd:.4}"),
        balance = format!("{new_balance_usd:.4}"),
        tx_hash = tx_hash,
    )
    .into_owned()
}

pub fn topup_not_configured(locale: &str) -> String {
    t!("billing.topup_not_configured", locale = locale).into_owned()
}

pub fn topup_amount_out_of_range(locale: &str, min_topup_usd: f64, max_topup_usd: f64) -> String {
    t!(
        "billing.topup_amount_out_of_range",
        locale = locale,
        min = format!("{min_topup_usd:.2}"),
        max = format!("{max_topup_usd:.2}"),
    )
    .into_owned()
}

pub fn topup_request_failed(locale: &str, error: &str) -> String {
    t!(
        "billing.topup_request_failed",
        locale = locale,
        error = error
    )
    .into_owned()
}

pub fn access_denied(locale: &str, command: &str) -> String {
    t!("billing.access_denied", locale = locale, command = command).into_owned()
}

pub fn invalid_command(locale: &str, command: &str, reason: &str) -> String {
    t!(
        "billing.invalid_command",
        locale = locale,
        command = command,
        reason = reason
    )
    .into_owned()
}

pub fn command_failed(locale: &str, command: &str, error: &str) -> String {
    t!(
        "billing.command_failed",
        locale = locale,
        command = command,
        error = error
    )
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_are_formatted_the_same_in_every_locale() {
        for locale in crate::i18n::available_locales() {
            let text = insufficient_balance_with_topup_command(&locale, 0.0123, 0.03, "!bai", 0.1);
            assert!(text.contains("$0.0123"), "{locale}: {text}");
            assert!(text.contains("$0.03"), "{locale}: {text}");
            assert!(text.contains("`!bai topup 0.10`"), "{locale}: {text}");
        }
    }

    #[test]
    fn cap_hit_names_the_period() {
        let daily = cap_hit("en", CapPeriod::Daily, 50.0, "2026-05-03T00:00:00+00:00");
        let monthly = cap_hit(
            "en",
            CapPeriod::Monthly,
            1000.0,
            "2026-06-01T00:00:00+00:00",
        );
        assert!(
            daily.contains("daily") && daily.contains("$50.00"),
            "{daily}"
        );
        assert!(
            monthly.contains("monthly") && monthly.contains("$1000.00"),
            "{monthly}"
        );
        assert!(daily.contains("2026-05-03T00:00:00+00:00"), "{daily}");
    }

    #[test]
    fn replies_follow_the_locale() {
        assert!(temporarily_unavailable("en").starts_with("Billing is temporarily unavailable"));
        assert!(temporarily_unavailable("ru").starts_with("Биллинг временно недоступен"));
        assert!(access_denied("de", "stats day").contains("`stats day`"));
        assert!(access_denied("de", "stats day").contains("Zugriff verweigert"));
    }

    #[test]
    fn unknown_locale_gets_english() {
        assert_eq!(topup_not_configured("xx"), topup_not_configured("en"));
    }

    /// rust-i18n does not fail on a placeholder without a matching argument: it leaves `%{name}`
    /// in the text. Render every reply in every locale and make sure nothing is left unfilled.
    #[test]
    fn every_reply_fills_all_placeholders_in_every_locale() {
        for locale in crate::i18n::available_locales() {
            let replies = [
                temporarily_unavailable(&locale),
                insufficient_balance(&locale, 0.0123, 0.03),
                insufficient_balance_with_widget(&locale, 0.0123, 0.03),
                insufficient_balance_with_topup_command(&locale, 0.0123, 0.03, "!bai", 0.1),
                cap_hit(&locale, CapPeriod::Daily, 50.0, "2026-05-03T00:00:00+00:00"),
                cap_hit(
                    &locale,
                    CapPeriod::Monthly,
                    1000.0,
                    "2026-06-01T00:00:00+00:00",
                ),
                topup_confirmed(&locale, 1.0, 1.0123, "abcdef12…3456"),
                topup_not_configured(&locale),
                topup_amount_out_of_range(&locale, 0.1, 1.0),
                topup_request_failed(&locale, "connection refused"),
                access_denied(&locale, "stats day"),
                invalid_command(&locale, "topup", "not a number"),
                command_failed(&locale, "balance", "ledger closed"),
            ];
            for reply in replies {
                assert!(
                    !reply.contains("%{"),
                    "{locale}: unfilled placeholder in {reply:?}"
                );
                assert!(!reply.trim().is_empty(), "{locale}: empty reply");
            }
        }
    }
}
