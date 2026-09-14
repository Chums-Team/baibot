//! Parser for the billing chat commands.
//!
//! Pure: takes the text after the command prefix plus the sender's access level and returns
//! a `BillingControllerType`. Routing (where `<prefix>` is the bot's command prefix):
//!
//! - `<prefix> balance`                                       → Balance
//! - `<prefix> topup` / `<prefix> topup <amount_usd>`         → Topup
//! - `<prefix> stats day` / `... month`                       → StatsDay / StatsMonth (administrators)
//! - `<prefix> billing` / `<prefix> billing help`             → Help
//! - `<prefix> billing zombies [<minutes>]`                   → Zombies (administrators)
//! - `<prefix> billing manual-release <event_id> "<reason>"`  → ManualRelease (administrators)
//! - `<prefix> billing manual-refund <room> <amt> "<reason>"` → ManualRefund (administrators)
//!
//! An administrator-only command from a non-administrator yields `AccessDenied` rather than
//! `ParseError`, so the reply is a clear refusal instead of a usage hint. When billing is not
//! configured, nothing is recognised and the text is routed as if the commands did not exist.

use uuid::Uuid;

use super::controller_type::{BillingCommandAccess, BillingControllerType};

/// Heads of the billing command family, as they follow the command prefix.
pub const COMMAND_HEADS: &[&str] = &["balance", "topup", "stats", "billing"];

const DEFAULT_ZOMBIE_AGE_MINUTES: u32 = 10;

/// `remaining` is the text after the command prefix (`"balance"`, `"stats day"`, ...).
pub fn determine_controller(
    remaining: &str,
    access: BillingCommandAccess,
) -> Option<BillingControllerType> {
    if access == BillingCommandAccess::Disabled {
        return None;
    }

    let is_admin = access.is_admin();
    let remaining = remaining.trim();

    if let Some(rest) = remaining.strip_prefix("balance") {
        return Some(if rest.trim().is_empty() {
            BillingControllerType::Balance
        } else {
            BillingControllerType::ParseError {
                command: "balance".into(),
                reason: "the command takes no arguments".into(),
            }
        });
    }

    if let Some(rest) = remaining.strip_prefix("topup") {
        // The range check against `billing.min_topup_usd`/`max_topup_usd` happens at dispatch
        // time, where the configuration is at hand. Here only the syntax is checked.
        let rest = rest.trim();
        if rest.is_empty() {
            return Some(BillingControllerType::Topup { amount_usd: None });
        }

        return Some(match rest.parse::<f64>() {
            Ok(amount) if amount.is_finite() => BillingControllerType::Topup {
                amount_usd: Some(amount),
            },
            _ => BillingControllerType::ParseError {
                command: "topup".into(),
                reason: format!("expected an amount in USD (e.g. `topup 0.50`), got `{rest}`"),
            },
        });
    }

    if let Some(rest) = remaining.strip_prefix("stats") {
        if !is_admin {
            return Some(BillingControllerType::AccessDenied { command: "stats" });
        }

        return Some(match rest.trim() {
            "day" => BillingControllerType::StatsDay,
            "month" => BillingControllerType::StatsMonth,
            "" => BillingControllerType::ParseError {
                command: "stats".into(),
                reason: "expected `day` or `month` (e.g. `stats day`)".into(),
            },
            other => BillingControllerType::ParseError {
                command: "stats".into(),
                reason: format!("unknown period `{other}` (use `day` or `month`)"),
            },
        });
    }

    if let Some(rest) = remaining.strip_prefix("billing") {
        return Some(parse_billing_subcommand(rest.trim(), is_admin));
    }

    None
}

fn parse_billing_subcommand(rest: &str, is_admin: bool) -> BillingControllerType {
    if rest.is_empty() || rest == "help" {
        return BillingControllerType::Help;
    }

    if let Some(args) = rest.strip_prefix("zombies") {
        if !is_admin {
            return BillingControllerType::AccessDenied {
                command: "billing zombies",
            };
        }

        let args = args.trim();
        if args.is_empty() {
            return BillingControllerType::Zombies {
                older_than_minutes: DEFAULT_ZOMBIE_AGE_MINUTES,
            };
        }

        return match args.parse::<u32>() {
            Ok(minutes) => BillingControllerType::Zombies {
                older_than_minutes: minutes,
            },
            Err(_) => BillingControllerType::ParseError {
                command: "billing zombies".into(),
                reason: format!("expected an integer number of minutes, got `{args}`"),
            },
        };
    }

    if let Some(args) = rest.strip_prefix("manual-release") {
        if !is_admin {
            return BillingControllerType::AccessDenied {
                command: "billing manual-release",
            };
        }

        // Format: `<event_id> "<reason>"`
        let (id_part, reason) = match split_id_and_quoted_reason(args) {
            Ok(parts) => parts,
            Err(reason) => {
                return BillingControllerType::ParseError {
                    command: "billing manual-release".into(),
                    reason,
                };
            }
        };

        let reserve_event_id = match Uuid::parse_str(&id_part) {
            Ok(uuid) => uuid,
            Err(_) => {
                return BillingControllerType::ParseError {
                    command: "billing manual-release".into(),
                    reason: format!("event_id `{id_part}` is not a valid UUID"),
                };
            }
        };

        return BillingControllerType::ManualRelease {
            reserve_event_id,
            reason,
        };
    }

    if let Some(args) = rest.strip_prefix("manual-refund") {
        if !is_admin {
            return BillingControllerType::AccessDenied {
                command: "billing manual-refund",
            };
        }

        // Format: `<room_id> <amount_usd> "<reason>"`
        let (head, reason) = match split_head_and_quoted_reason(args) {
            Ok(parts) => parts,
            Err(reason) => {
                return BillingControllerType::ParseError {
                    command: "billing manual-refund".into(),
                    reason,
                };
            }
        };

        let mut parts = head.split_whitespace();

        let Some(room_id) = parts.next() else {
            return BillingControllerType::ParseError {
                command: "billing manual-refund".into(),
                reason: "missing <room_id>".into(),
            };
        };

        let Some(amount_str) = parts.next() else {
            return BillingControllerType::ParseError {
                command: "billing manual-refund".into(),
                reason: "missing <amount_usd>".into(),
            };
        };

        let amount_usd = match amount_str.parse::<f64>() {
            Ok(amount) if amount.is_finite() && amount > 0.0 => amount,
            _ => {
                return BillingControllerType::ParseError {
                    command: "billing manual-refund".into(),
                    reason: format!("amount `{amount_str}` must be a positive number"),
                };
            }
        };

        if parts.next().is_some() {
            return BillingControllerType::ParseError {
                command: "billing manual-refund".into(),
                reason: "extra tokens before the quoted reason".into(),
            };
        }

        return BillingControllerType::ManualRefund {
            room_id: room_id.to_owned(),
            amount_usd,
            reason,
        };
    }

    BillingControllerType::ParseError {
        command: "billing".into(),
        reason: format!("unknown sub-command `{rest}`"),
    }
}

/// Splits `<id> "<reason>"` into `(id, reason)`, with the reason unquoted.
fn split_id_and_quoted_reason(s: &str) -> Result<(String, String), String> {
    let s = s.trim();
    let first_space = s
        .find(char::is_whitespace)
        .ok_or_else(|| "missing reason (usage: `<event_id> \"<reason>\"`)".to_owned())?;
    let id = s[..first_space].to_owned();
    let reason = extract_quoted(&s[first_space..])?;
    Ok((id, reason))
}

/// Splits `<head...> "<reason>"` into the head (everything before the opening quote, trimmed)
/// and the unquoted reason.
fn split_head_and_quoted_reason(s: &str) -> Result<(String, String), String> {
    let s = s.trim();
    let quote_at = s
        .find('"')
        .ok_or_else(|| "missing quoted reason (usage: `... \"<reason>\"`)".to_owned())?;
    let head = s[..quote_at].trim().to_owned();
    let reason = extract_quoted(&s[quote_at..])?;
    Ok((head, reason))
}

fn extract_quoted(s: &str) -> Result<String, String> {
    let s = s.trim();
    let stripped = s
        .strip_prefix('"')
        .ok_or_else(|| "reason must start with `\"`".to_owned())?;
    let inner = stripped
        .strip_suffix('"')
        .ok_or_else(|| "reason must end with `\"`".to_owned())?;
    if inner.is_empty() {
        return Err("reason cannot be empty".into());
    }
    Ok(inner.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: BillingCommandAccess = BillingCommandAccess::User;
    const ADMIN: BillingCommandAccess = BillingCommandAccess::Admin;

    fn assert_access_denied(result: Option<BillingControllerType>, expected_command: &str) {
        match result {
            Some(BillingControllerType::AccessDenied { command }) => {
                assert_eq!(command, expected_command)
            }
            other => panic!("expected AccessDenied, got {other:?}"),
        }
    }

    fn assert_parse_error(result: Option<BillingControllerType>, expected_command: &str) {
        match result {
            Some(BillingControllerType::ParseError { command, .. }) => {
                assert_eq!(command, expected_command)
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn disabled_recognises_nothing() {
        for text in ["balance", "stats day", "billing", "billing zombies"] {
            assert_eq!(
                determine_controller(text, BillingCommandAccess::Disabled),
                None,
                "{text}"
            );
        }
    }

    #[test]
    fn balance_public() {
        assert_eq!(
            determine_controller("balance", USER),
            Some(BillingControllerType::Balance)
        );
        assert_eq!(
            determine_controller("  balance  ", ADMIN),
            Some(BillingControllerType::Balance)
        );
    }

    #[test]
    fn balance_with_arguments_parse_error() {
        assert_parse_error(determine_controller("balance now", USER), "balance");
    }

    #[test]
    fn topup_public_default_amount() {
        assert_eq!(
            determine_controller("topup", USER),
            Some(BillingControllerType::Topup { amount_usd: None })
        );
        assert_eq!(
            determine_controller("  topup  ", ADMIN),
            Some(BillingControllerType::Topup { amount_usd: None })
        );
    }

    #[test]
    fn topup_public_with_amount() {
        // Any finite number is accepted here; the configured bounds are applied at dispatch.
        for (text, expected) in [("topup 0.50", 0.50), ("topup 0", 0.0), ("topup -1", -1.0)] {
            match determine_controller(text, USER) {
                Some(BillingControllerType::Topup {
                    amount_usd: Some(amount),
                }) => assert!((amount - expected).abs() < 1e-12, "{text}"),
                other => panic!("{text}: expected Topup, got {other:?}"),
            }
        }
    }

    #[test]
    fn topup_non_numeric_amount_parse_error() {
        assert_parse_error(determine_controller("topup abc", USER), "topup");
        assert_parse_error(determine_controller("topup NaN", USER), "topup");
        assert_parse_error(determine_controller("topup 1 2", USER), "topup");
    }

    #[test]
    fn stats_admin() {
        assert_eq!(
            determine_controller("stats day", ADMIN),
            Some(BillingControllerType::StatsDay)
        );
        assert_eq!(
            determine_controller("stats month", ADMIN),
            Some(BillingControllerType::StatsMonth)
        );
    }

    #[test]
    fn stats_non_admin_denied() {
        assert_access_denied(determine_controller("stats day", USER), "stats");
    }

    #[test]
    fn stats_missing_or_unknown_period_parse_error() {
        assert_parse_error(determine_controller("stats", ADMIN), "stats");
        assert_parse_error(determine_controller("stats year", ADMIN), "stats");
    }

    #[test]
    fn billing_help_public() {
        assert_eq!(
            determine_controller("billing", USER),
            Some(BillingControllerType::Help)
        );
        assert_eq!(
            determine_controller("billing help", USER),
            Some(BillingControllerType::Help)
        );
    }

    #[test]
    fn billing_zombies_admin_default_minutes() {
        assert_eq!(
            determine_controller("billing zombies", ADMIN),
            Some(BillingControllerType::Zombies {
                older_than_minutes: DEFAULT_ZOMBIE_AGE_MINUTES
            })
        );
    }

    #[test]
    fn billing_zombies_admin_custom_minutes() {
        assert_eq!(
            determine_controller("billing zombies 60", ADMIN),
            Some(BillingControllerType::Zombies {
                older_than_minutes: 60
            })
        );
    }

    #[test]
    fn billing_zombies_non_admin_denied() {
        assert_access_denied(
            determine_controller("billing zombies", USER),
            "billing zombies",
        );
    }

    #[test]
    fn billing_zombies_bad_arg_parse_error() {
        assert_parse_error(
            determine_controller("billing zombies abc", ADMIN),
            "billing zombies",
        );
    }

    #[test]
    fn billing_manual_release_admin() {
        let id = Uuid::new_v4();
        let text = format!(r#"billing manual-release {id} "stuck after upgrade""#);
        match determine_controller(&text, ADMIN) {
            Some(BillingControllerType::ManualRelease {
                reserve_event_id,
                reason,
            }) => {
                assert_eq!(reserve_event_id, id);
                assert_eq!(reason, "stuck after upgrade");
            }
            other => panic!("expected ManualRelease, got {other:?}"),
        }
    }

    #[test]
    fn billing_manual_release_non_admin_denied() {
        assert_access_denied(
            determine_controller(r#"billing manual-release 0192-... "x""#, USER),
            "billing manual-release",
        );
    }

    #[test]
    fn billing_manual_release_bad_uuid_parse_error() {
        assert_parse_error(
            determine_controller(r#"billing manual-release nope "reason""#, ADMIN),
            "billing manual-release",
        );
    }

    #[test]
    fn billing_manual_release_missing_reason_parse_error() {
        let id = Uuid::new_v4();
        assert_parse_error(
            determine_controller(&format!("billing manual-release {id}"), ADMIN),
            "billing manual-release",
        );
        assert_parse_error(
            determine_controller(&format!("billing manual-release {id} unquoted"), ADMIN),
            "billing manual-release",
        );
        assert_parse_error(
            determine_controller(&format!(r#"billing manual-release {id} """#), ADMIN),
            "billing manual-release",
        );
    }

    #[test]
    fn billing_manual_refund_admin() {
        let text = r#"billing manual-refund !room:example.com 1.50 "duplicate topup""#;
        match determine_controller(text, ADMIN) {
            Some(BillingControllerType::ManualRefund {
                room_id,
                amount_usd,
                reason,
            }) => {
                assert_eq!(room_id, "!room:example.com");
                assert!((amount_usd - 1.50).abs() < 1e-12);
                assert_eq!(reason, "duplicate topup");
            }
            other => panic!("expected ManualRefund, got {other:?}"),
        }
    }

    #[test]
    fn billing_manual_refund_non_admin_denied() {
        assert_access_denied(
            determine_controller(r#"billing manual-refund !r 1 "x""#, USER),
            "billing manual-refund",
        );
    }

    #[test]
    fn billing_manual_refund_bad_arguments_parse_error() {
        for text in [
            r#"billing manual-refund !r free "reason""#,
            r#"billing manual-refund !r -1 "reason""#,
            r#"billing manual-refund !r NaN "reason""#,
            r#"billing manual-refund !r "reason""#,
            r#"billing manual-refund "reason""#,
            r#"billing manual-refund !r 1 extra "reason""#,
            r#"billing manual-refund !r 1"#,
        ] {
            assert_parse_error(determine_controller(text, ADMIN), "billing manual-refund");
        }
    }

    #[test]
    fn billing_unknown_subcommand_parse_error() {
        assert_parse_error(determine_controller("billing wat", ADMIN), "billing");
    }

    #[test]
    fn returns_none_on_unrelated_text() {
        assert_eq!(determine_controller("config foo", ADMIN), None);
        assert_eq!(determine_controller("hello world", ADMIN), None);
    }
}
