#[cfg(test)]
mod tests;

use super::billing::BillingCommandAccess;
use super::chat_completion::ChatCompletionControllerType;
use crate::{
    entity::{
        InteractionTrigger, MessageContext, MessagePayload, cfg::ConfigAccess,
        roomconfig::TextGenerationPrefixRequirementType,
    },
    strings,
};

use super::ControllerType;

/// The first words of the bot's commands (`<command_prefix> <head> …`), as
/// `access.commands_admin_exempt` names them. The billing heads exist only when billing is
/// configured, see `controller::billing::COMMAND_HEADS`.
pub const COMMAND_HEADS: &[&str] = &[
    "help", "access", "provider", "agent", "config", "image", "sticker", "usage", "balance",
    "topup", "stats", "billing",
];

/// `access.commands_admin_only` for one message: which commands the sender may use.
/// Conversation (plain text, mentions, `<command_prefix> <free text>`) is never subject to it.
#[derive(Debug, Clone, Copy)]
pub struct CommandGate<'a> {
    sender_is_admin: bool,
    access_config: &'a ConfigAccess,
}

impl<'a> CommandGate<'a> {
    pub fn new(sender_is_admin: bool, access_config: &'a ConfigAccess) -> Self {
        Self {
            sender_is_admin,
            access_config,
        }
    }

    /// Returns `controller_type` when the sender may use the command `head`, otherwise
    /// `ControllerType::Ignore` (the command is dropped without a reply).
    fn apply(&self, head: &str, controller_type: ControllerType) -> ControllerType {
        if self.sender_is_admin || !self.access_config.is_command_admin_only(head) {
            return controller_type;
        }

        tracing::debug!(
            head,
            "Ignoring command from a non-administrator (access.commands_admin_only)"
        );

        ControllerType::Ignore
    }
}

pub fn determine_controller(
    command_prefix: &str,
    first_thread_message: &InteractionTrigger,
    message_context: &MessageContext,
    billing_access: BillingCommandAccess,
    access_config: &ConfigAccess,
) -> ControllerType {
    let command_gate = CommandGate::new(
        message_context.sender_can_manage_global_config(),
        access_config,
    );

    match &first_thread_message.payload {
        MessagePayload::SynthethicChatCompletionTriggerInThread => {
            ControllerType::ChatCompletion(ChatCompletionControllerType::ThreadMention)
        }
        MessagePayload::SynthethicChatCompletionTriggerForReply => {
            ControllerType::ChatCompletion(ChatCompletionControllerType::ReplyMention)
        }
        MessagePayload::Text(text_message_content) => {
            let prefix_requirement_type = message_context
                .room_config_context()
                .text_generation_prefix_requirement_type();

            determine_text_controller(
                command_prefix,
                &text_message_content.body,
                prefix_requirement_type,
                first_thread_message.is_mentioning_bot,
                billing_access,
                &command_gate,
            )
        }
        MessagePayload::Image(_image_message_content) => {
            let prefix_requirement_type = message_context
                .room_config_context()
                .text_generation_prefix_requirement_type();

            match prefix_requirement_type {
                TextGenerationPrefixRequirementType::CommandPrefix => ControllerType::Ignore,
                TextGenerationPrefixRequirementType::No => {
                    ControllerType::ChatCompletion(ChatCompletionControllerType::Image)
                }
            }
        }
        MessagePayload::Encrypted(thread_info) => {
            if thread_info.is_thread_root_only() {
                ControllerType::Error(strings::error::message_is_encrypted().to_owned())
            } else {
                ControllerType::ErrorInThread(
                    strings::error::first_message_in_thread_is_encrypted().to_owned(),
                    thread_info.clone(),
                )
            }
        }
        MessagePayload::File(_file_message_content) => {
            let prefix_requirement_type = message_context
                .room_config_context()
                .text_generation_prefix_requirement_type();

            match prefix_requirement_type {
                TextGenerationPrefixRequirementType::CommandPrefix => ControllerType::Ignore,
                TextGenerationPrefixRequirementType::No => {
                    ControllerType::ChatCompletion(ChatCompletionControllerType::File)
                }
            }
        }
        MessagePayload::Audio(_) => {
            ControllerType::ChatCompletion(ChatCompletionControllerType::Audio)
        }
        MessagePayload::Reaction { .. } => {
            panic!("Handling reaction as first message in thread does not make sense")
        }
    }
}

fn determine_text_controller(
    command_prefix: &str,
    text: &str,
    room_text_generation_prefix_requirement_type: TextGenerationPrefixRequirementType,
    is_mentioning_bot: bool,
    billing_access: BillingCommandAccess,
    command_gate: &CommandGate,
) -> ControllerType {
    let text = text.trim();

    if text.starts_with(&format!("{command_prefix} help")) || text == command_prefix {
        return command_gate.apply("help", ControllerType::Help);
    }

    if let Some(remaining) = text.strip_prefix(&format!("{command_prefix} access")) {
        return command_gate.apply(
            "access",
            super::access::determine_controller(remaining.trim()),
        );
    }

    if let Some(remaining) = text.strip_prefix(&format!("{command_prefix} provider")) {
        return command_gate.apply(
            "provider",
            super::provider::determine_controller(remaining.trim()),
        );
    }

    if let Some(remaining) = text.strip_prefix(&format!("{command_prefix} agent")) {
        return command_gate.apply(
            "agent",
            super::agent::determine_controller(command_prefix, remaining.trim()),
        );
    }

    if let Some(remaining) = text.strip_prefix(&format!("{command_prefix} config")) {
        return command_gate.apply("config", super::cfg::determine_controller(remaining.trim()));
    }

    if let Some(prompt) = text.strip_prefix(&format!("{command_prefix} image")) {
        return command_gate.apply("image", super::image::determine_controller(prompt.trim()));
    }

    if let Some(prompt) = text.strip_prefix(&format!("{command_prefix} sticker")) {
        return command_gate.apply(
            "sticker",
            ControllerType::StickerGeneration(prompt.trim().to_owned()),
        );
    }

    if let Some(remaining) = text.strip_prefix(&format!("{command_prefix} usage")) {
        return command_gate.apply(
            "usage",
            super::usage::determine_controller(remaining.trim()),
        );
    }

    // Billing commands exist only when billing is configured (`billing_access` is not `Disabled`).
    // Otherwise the text falls through and is treated like upstream does: as a chat completion.
    if let Some((head, remaining)) = strip_billing_command(command_prefix, text)
        && let Some(billing_controller_type) =
            super::billing::determine_controller(remaining, billing_access)
    {
        return command_gate.apply(head, ControllerType::Billing(billing_controller_type));
    }

    // Regular text message that does not match any command.
    // If it mentions the bot, it's a chat completion.
    // Otherwise, it depends on the prefix requirement for text generation - it may be routed for chat completion or ignored.

    if is_mentioning_bot {
        return ControllerType::ChatCompletion(ChatCompletionControllerType::TextMention);
    }

    // Regardless of what the prefix requirement is, if we encounter a command prefix, we'll consider it a chat completion via command prefix invokation.
    // This is to correctly indicate to the chat completion controller that a command prefix was used,
    // so that it can be stripped from the beginning of the message.
    if text.starts_with(command_prefix) {
        return ControllerType::ChatCompletion(ChatCompletionControllerType::TextCommand);
    }

    // We're dealing with a regular message that does not start with a command prefix.

    match room_text_generation_prefix_requirement_type {
        TextGenerationPrefixRequirementType::CommandPrefix => {
            // A prefix is required, but we've already checked (above) that the message does not start with a command prefix.
            // It's to be ignored.
            ControllerType::Ignore
        }
        TextGenerationPrefixRequirementType::No => {
            ControllerType::ChatCompletion(ChatCompletionControllerType::TextDirect)
        }
    }
}

/// Returns the head and the text after the command prefix when it starts with one of the
/// billing command heads (`balance`, `topup`, `stats`, `billing`) as a whole word, so that the
/// billing parser sees e.g. `stats day`. Free-form text after the prefix is not a billing
/// command.
fn strip_billing_command<'a>(command_prefix: &str, text: &'a str) -> Option<(&'a str, &'a str)> {
    let remaining = text
        .strip_prefix(command_prefix)?
        .strip_prefix(char::is_whitespace)?
        .trim_start();

    let head = remaining.split_whitespace().next()?;

    super::billing::COMMAND_HEADS
        .contains(&head)
        .then_some((head, remaining))
}
