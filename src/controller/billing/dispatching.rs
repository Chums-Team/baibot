//! Runs a parsed `BillingControllerType` against the live bot: calls the matching handler
//! and sends its markdown as a reply in the thread.

use mxlink::MessageResponseType;

use super::controller_type::{BillingCommandAccess, BillingControllerType};
use super::handlers;
use crate::billing::{BillingContext, Period};
use crate::entity::MessageContext;
use crate::{Bot, strings};

pub async fn dispatch_controller(
    controller_type: &BillingControllerType,
    message_context: &MessageContext,
    bot: &Bot,
) -> anyhow::Result<()> {
    let Some(billing) = bot.billing() else {
        anyhow::bail!("A billing command was routed while billing is not configured");
    };

    let response_type =
        MessageResponseType::Reply(message_context.thread_info().root_event_id.clone());

    match run(controller_type, message_context, bot, billing).await {
        Ok(reply) => {
            bot.messaging()
                .send_text_markdown_no_fail(message_context.room(), reply, response_type)
                .await;
        }
        Err(reply) => {
            bot.messaging()
                .send_error_markdown_no_fail(message_context.room(), &reply, response_type)
                .await;
        }
    }

    Ok(())
}

/// `Ok` is a regular reply, `Err` is a reply to be sent as an error notice.
async fn run(
    controller_type: &BillingControllerType,
    message_context: &MessageContext,
    bot: &Bot,
    billing: &BillingContext,
) -> Result<String, String> {
    let sender_id = message_context.sender_id();
    let is_admin = BillingCommandAccess::determine(bot.billing_config(), sender_id).is_admin();

    match controller_type {
        BillingControllerType::Help => Ok(handlers::help(bot.command_prefix(), is_admin)),

        BillingControllerType::Balance => {
            handlers::balance(&billing.service, message_context.room_id().as_str(), 5)
                .await
                .map_err(|e| strings::billing::command_failed("balance", &e.to_string()))
        }

        BillingControllerType::StatsDay => handlers::stats(
            &billing.service,
            Period::Day,
            billing.wrapper_config.daily_cap_usd,
        )
        .await
        .map_err(|e| strings::billing::command_failed("stats day", &e.to_string())),

        BillingControllerType::StatsMonth => handlers::stats(
            &billing.service,
            Period::Month,
            billing.wrapper_config.monthly_cap_usd,
        )
        .await
        .map_err(|e| strings::billing::command_failed("stats month", &e.to_string())),

        BillingControllerType::Zombies { older_than_minutes } => {
            handlers::list_zombies(&billing.service, *older_than_minutes, bot.command_prefix())
                .await
                .map_err(|e| strings::billing::command_failed("billing zombies", &e.to_string()))
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
        .map_err(|e| strings::billing::command_failed("billing manual-release", &e.to_string())),

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
        .map_err(|e| strings::billing::command_failed("billing manual-refund", &e.to_string())),

        BillingControllerType::AccessDenied { command } => {
            Err(strings::billing::access_denied(command))
        }

        BillingControllerType::ParseError { command, reason } => Err(format!(
            "{}\n\n{}",
            strings::billing::invalid_command(command, reason),
            handlers::help(bot.command_prefix(), is_admin),
        )),
    }
}
