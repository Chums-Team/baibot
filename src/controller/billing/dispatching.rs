//! Runs a parsed `BillingControllerType` against the live bot: calls the matching handler
//! and sends its markdown as a reply in the thread.

use mxlink::MessageResponseType;

use super::controller_type::{BillingCommandAccess, BillingControllerType};
use super::handlers;
use crate::billing::{BillingContext, Period};
use crate::entity::MessageContext;
use crate::entity::cfg::ConfigBilling;
use crate::matrix::events::{X402RequestContent, emit_x402_request};
use crate::{Bot, strings};

pub async fn dispatch_controller(
    controller_type: &BillingControllerType,
    message_context: &MessageContext,
    bot: &Bot,
) -> anyhow::Result<()> {
    let (Some(billing), Some(billing_config)) = (bot.billing(), bot.billing_config()) else {
        anyhow::bail!("A billing command was routed while billing is not configured");
    };

    let response_type =
        MessageResponseType::Reply(message_context.thread_info().root_event_id.clone());

    match run(
        controller_type,
        message_context,
        bot,
        billing,
        billing_config,
    )
    .await
    {
        Reply::Text(reply) => {
            bot.messaging()
                .send_text_markdown_no_fail(message_context.room(), reply, response_type)
                .await;
        }
        Reply::Error(reply) => {
            bot.messaging()
                .send_error_markdown_no_fail(message_context.room(), &reply, response_type)
                .await;
        }
        Reply::X402Request { text, content } => {
            // The text goes first so that it precedes the widget in the timeline.
            bot.messaging()
                .send_text_markdown_no_fail(message_context.room(), text, response_type)
                .await;

            if let Err(e) = emit_x402_request(message_context.room(), &content).await {
                tracing::warn!(error = %e, "emit_x402_request failed for the topup command");
            }
        }
    }

    Ok(())
}

enum Reply {
    /// A regular reply.
    Text(String),
    /// A reply sent as an error notice.
    Error(String),
    /// A text for clients without the payment widget, followed by the widget event.
    X402Request {
        text: String,
        content: Box<X402RequestContent>,
    },
}

async fn run(
    controller_type: &BillingControllerType,
    message_context: &MessageContext,
    bot: &Bot,
    billing: &BillingContext,
    billing_config: &ConfigBilling,
) -> Reply {
    let sender_id = message_context.sender_id();
    let is_admin = BillingCommandAccess::determine(Some(billing_config), sender_id).is_admin();
    let topup_available = bot.x402_client().is_some();
    let locale = bot.resolve_user_locale(sender_id).await;
    let locale = locale.as_str();

    match controller_type {
        BillingControllerType::Help => Reply::Text(handlers::help(
            locale,
            bot.command_prefix(),
            is_admin,
            topup_available,
        )),

        BillingControllerType::Balance => handlers::balance(
            locale,
            &billing.service,
            message_context.room_id().as_str(),
            5,
        )
        .await
        .map_or_else(
            |e| {
                Reply::Error(strings::billing::command_failed(
                    locale,
                    "balance",
                    &e.to_string(),
                ))
            },
            Reply::Text,
        ),

        BillingControllerType::Topup { amount_usd } => {
            let Some(x402_client) = bot.x402_client() else {
                return Reply::Error(strings::billing::topup_not_configured(locale));
            };

            let amount_usd = amount_usd.unwrap_or(billing_config.min_topup_usd);
            if amount_usd < billing_config.min_topup_usd
                || amount_usd > billing_config.max_topup_usd
            {
                return Reply::Error(strings::billing::topup_amount_out_of_range(
                    locale,
                    billing_config.min_topup_usd,
                    billing_config.max_topup_usd,
                ));
            }

            match x402_client
                .create_x402_request_content(
                    message_context.room_id().as_str(),
                    sender_id.as_str(),
                    amount_usd,
                    billing_config.min_topup_usd,
                    bot.command_prefix(),
                )
                .await
            {
                Ok(content) => Reply::X402Request {
                    text: handlers::topup_invoice(locale, amount_usd),
                    content: Box::new(content),
                },
                Err(e) => {
                    tracing::warn!(error = %e, "The payment sidecar refused a topup request");
                    Reply::Error(strings::billing::topup_request_failed(
                        locale,
                        &e.to_string(),
                    ))
                }
            }
        }

        BillingControllerType::StatsDay => handlers::stats(
            &billing.service,
            Period::Day,
            billing.wrapper_config.daily_cap_usd,
        )
        .await
        .map_or_else(
            |e| {
                Reply::Error(strings::billing::command_failed(
                    locale,
                    "stats day",
                    &e.to_string(),
                ))
            },
            Reply::Text,
        ),

        BillingControllerType::StatsMonth => handlers::stats(
            &billing.service,
            Period::Month,
            billing.wrapper_config.monthly_cap_usd,
        )
        .await
        .map_or_else(
            |e| {
                Reply::Error(strings::billing::command_failed(
                    locale,
                    "stats month",
                    &e.to_string(),
                ))
            },
            Reply::Text,
        ),

        BillingControllerType::Zombies { older_than_minutes } => {
            handlers::list_zombies(&billing.service, *older_than_minutes, bot.command_prefix())
                .await
                .map_or_else(
                    |e| {
                        Reply::Error(strings::billing::command_failed(
                            locale,
                            "billing zombies",
                            &e.to_string(),
                        ))
                    },
                    Reply::Text,
                )
        }

        BillingControllerType::ManualRelease {
            reserve_event_id,
            reason,
        } => handlers::manual_release(
            &billing.service,
            *reserve_event_id,
            sender_id.as_str(),
            reason,
        )
        .await
        .map_or_else(
            |e| {
                Reply::Error(strings::billing::command_failed(
                    locale,
                    "billing manual-release",
                    &e.to_string(),
                ))
            },
            Reply::Text,
        ),

        BillingControllerType::ManualRefund {
            room_id,
            amount_usd,
            reason,
        } => handlers::manual_refund(
            &billing.service,
            room_id,
            *amount_usd,
            sender_id.as_str(),
            reason,
        )
        .await
        .map_or_else(
            |e| {
                Reply::Error(strings::billing::command_failed(
                    locale,
                    "billing manual-refund",
                    &e.to_string(),
                ))
            },
            Reply::Text,
        ),

        BillingControllerType::AccessDenied { command } => {
            Reply::Error(strings::billing::access_denied(locale, command))
        }

        BillingControllerType::ParseError { command, reason } => Reply::Error(format!(
            "{}\n\n{}",
            strings::billing::invalid_command(locale, command, reason),
            handlers::help(locale, bot.command_prefix(), is_admin, topup_available),
        )),
    }
}
