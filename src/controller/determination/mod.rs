#[cfg(test)]
mod tests;

use super::billing::BillingCommandAccess;
use super::chat_completion::ChatCompletionControllerType;
use crate::{
    entity::{
        InteractionTrigger, MessageContext, MessagePayload,
        roomconfig::TextGenerationPrefixRequirementType,
    },
    strings,
};

use super::ControllerType;

pub fn determine_controller(
    command_prefix: &str,
    first_thread_message: &InteractionTrigger,
    message_context: &MessageContext,
    billing_access: BillingCommandAccess,
) -> ControllerType {
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
) -> ControllerType {
    let text = text.trim();

    if text.starts_with(&format!("{command_prefix} help")) || text == command_prefix {
        return ControllerType::Help;
    }

    if let Some(remaining) = text.strip_prefix(&format!("{command_prefix} access")) {
        return super::access::determine_controller(remaining.trim());
    }

    if let Some(remaining) = text.strip_prefix(&format!("{command_prefix} provider")) {
        return super::provider::determine_controller(remaining.trim());
    }

    if let Some(remaining) = text.strip_prefix(&format!("{command_prefix} agent")) {
        return super::agent::determine_controller(command_prefix, remaining.trim());
    }

    if let Some(remaining) = text.strip_prefix(&format!("{command_prefix} config")) {
        return super::cfg::determine_controller(remaining.trim());
    }

    if let Some(prompt) = text.strip_prefix(&format!("{command_prefix} image")) {
        return super::image::determine_controller(prompt.trim());
    }

    if let Some(prompt) = text.strip_prefix(&format!("{command_prefix} sticker")) {
        return ControllerType::StickerGeneration(prompt.trim().to_owned());
    }

    if let Some(remaining) = text.strip_prefix(&format!("{command_prefix} usage")) {
        return super::usage::determine_controller(remaining.trim());
    }

    // Billing commands exist only when billing is configured (`billing_access` is not `Disabled`).
    // Otherwise the text falls through and is treated like upstream does: as a chat completion.
    if let Some(remaining) = strip_billing_command(command_prefix, text)
        && let Some(billing_controller_type) =
            super::billing::determine_controller(remaining, billing_access)
    {
        return ControllerType::Billing(billing_controller_type);
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

/// Returns the text after the command prefix when it starts with one of the billing command
/// heads (`balance`, `stats`, `billing`) as a whole word, so that the billing parser sees
/// e.g. `stats day`. Free-form text after the prefix is not a billing command.
fn strip_billing_command<'a>(command_prefix: &str, text: &'a str) -> Option<&'a str> {
    let remaining = text
        .strip_prefix(command_prefix)?
        .strip_prefix(char::is_whitespace)?
        .trim_start();

    let head = remaining.split_whitespace().next()?;

    super::billing::COMMAND_HEADS
        .contains(&head)
        .then_some(remaining)
}
