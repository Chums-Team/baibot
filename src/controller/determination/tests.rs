use super::CommandGate;
use crate::entity::cfg::ConfigAccess;

fn access_config(commands_admin_only: bool, exempt: &[&str]) -> ConfigAccess {
    ConfigAccess {
        admin_patterns: vec!["@admin:example.com".to_owned()],
        commands_admin_only,
        commands_admin_exempt: exempt.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// A gate that lets every command through (`access.commands_admin_only` off).
fn open_gate() -> CommandGate<'static> {
    static OPEN: std::sync::OnceLock<ConfigAccess> = std::sync::OnceLock::new();
    CommandGate::new(false, OPEN.get_or_init(|| access_config(false, &[])))
}

#[test]
fn determine_text_controller() {
    use super::super::chat_completion::ChatCompletionControllerType;
    use super::ControllerType;
    use crate::controller;

    let command_prefix = "!bai";

    struct TestCase {
        name: &'static str,
        input: &'static str,
        is_mentioning_bot: bool,
        expected: ControllerType,
        // This value only matters for some of the tests.
        // We default to using the No variant for most tests where it's irrelevant.
        room_text_generation_prefix_requirement_type: super::TextGenerationPrefixRequirementType,
    }

    // We only have top-level test cases here.
    // Each submodule defines its own test cases.
    let test_cases = vec![
        TestCase {
            name: "Help",
            input: "!bai help",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::Help,
        },
        TestCase {
            name: "Prefix only leads to help",
            input: "!bai",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::Help,
        },
        TestCase {
            name: "Prefix and unknown command leads to chat completion",
            input: "!bai something-else",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::ChatCompletion(ChatCompletionControllerType::TextCommand),
        },
        TestCase {
            name: "Access top-level",
            input: "!bai access",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::Access(controller::access::AccessControllerType::Help),
        },
        TestCase {
            name: "Provider",
            input: "!bai provider",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::ProviderHelp,
        },
        TestCase {
            name: "Usage",
            input: "!bai usage",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::UsageHelp,
        },
        TestCase {
            name: "Agent top-level",
            input: "!bai agent",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::Agent(controller::agent::AgentControllerType::Help),
        },
        TestCase {
            name: "Config top-level",
            input: "!bai config",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::Config(controller::cfg::ConfigControllerType::Help),
        },
        TestCase {
            name: "Generic image command causes usage help",
            input: "!bai image Draw a cat!",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::UsageHelp,
        },
        TestCase {
            name: "Image generation",
            input: "!bai image create Draw a cat!",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::ImageGeneration("Draw a cat!".to_owned()),
        },
        TestCase {
            name: "Sticker generation",
            input: "!bai sticker A surprised cat",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::StickerGeneration("A surprised cat".to_owned()),
        },
        TestCase {
            name: "Regular text triggers completion when prefix not required",
            input: "Regular text goes here",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::ChatCompletion(ChatCompletionControllerType::TextDirect),
        },
        TestCase {
            name: "Regular text is ignored when prefix is required",
            input: "Regular text goes here",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::CommandPrefix,
            expected: ControllerType::Ignore,
        },
        TestCase {
            name: "Command-prefixed text triggers completion when prefix is required",
            input: "!bai Regular text goes here",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::CommandPrefix,
            expected: ControllerType::ChatCompletion(ChatCompletionControllerType::TextCommand),
        },
        TestCase {
            name: "Command-prefixed text triggers completion even when prefix is not required",
            input: "!bai Regular text goes here",
            is_mentioning_bot: false,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::ChatCompletion(ChatCompletionControllerType::TextCommand),
        },
        TestCase {
            name: "Regular message with bot mention triggers completion (no prefix requirement)",
            input: "Regular text goes here",
            is_mentioning_bot: true,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::No,
            expected: ControllerType::ChatCompletion(ChatCompletionControllerType::TextMention),
        },
        // This test case is the same as the one above, just with a different prefix requirement setting.
        // We expect the same result.
        TestCase {
            name: "Regular message with bot mention triggers completion (command prefix requirement)",
            input: "Regular text goes here",
            is_mentioning_bot: true,
            room_text_generation_prefix_requirement_type:
                super::TextGenerationPrefixRequirementType::CommandPrefix,
            expected: ControllerType::ChatCompletion(ChatCompletionControllerType::TextMention),
        },
    ];

    for test_case in test_cases {
        let result = super::determine_text_controller(
            command_prefix,
            test_case.input,
            test_case.room_text_generation_prefix_requirement_type,
            test_case.is_mentioning_bot,
            super::BillingCommandAccess::Disabled,
            &open_gate(),
        );
        assert_eq!(result, test_case.expected, "Test case: {}", test_case.name);
    }
}

#[test]
fn determine_text_controller_billing() {
    use super::super::billing::{BillingCommandAccess, BillingControllerType};
    use super::super::chat_completion::ChatCompletionControllerType;
    use super::ControllerType;

    let command_prefix = "!bai";

    struct TestCase {
        name: &'static str,
        input: &'static str,
        billing_access: BillingCommandAccess,
        expected: ControllerType,
    }

    let test_cases = vec![
        TestCase {
            name: "Balance is a chat completion when billing is not configured",
            input: "!bai balance",
            billing_access: BillingCommandAccess::Disabled,
            expected: ControllerType::ChatCompletion(ChatCompletionControllerType::TextCommand),
        },
        TestCase {
            name: "Billing help is a chat completion when billing is not configured",
            input: "!bai billing help",
            billing_access: BillingCommandAccess::Disabled,
            expected: ControllerType::ChatCompletion(ChatCompletionControllerType::TextCommand),
        },
        TestCase {
            name: "Balance",
            input: "!bai balance",
            billing_access: BillingCommandAccess::User,
            expected: ControllerType::Billing(BillingControllerType::Balance),
        },
        TestCase {
            name: "Balance with extra whitespace",
            input: "!bai   balance ",
            billing_access: BillingCommandAccess::User,
            expected: ControllerType::Billing(BillingControllerType::Balance),
        },
        TestCase {
            name: "Billing help",
            input: "!bai billing",
            billing_access: BillingCommandAccess::User,
            expected: ControllerType::Billing(BillingControllerType::Help),
        },
        TestCase {
            name: "Stats for an administrator",
            input: "!bai stats day",
            billing_access: BillingCommandAccess::Admin,
            expected: ControllerType::Billing(BillingControllerType::StatsDay),
        },
        TestCase {
            name: "Stats for a regular user",
            input: "!bai stats day",
            billing_access: BillingCommandAccess::User,
            expected: ControllerType::Billing(BillingControllerType::AccessDenied {
                command: "stats",
            }),
        },
        TestCase {
            name: "Zombies for an administrator",
            input: "!bai billing zombies 30",
            billing_access: BillingCommandAccess::Admin,
            expected: ControllerType::Billing(BillingControllerType::Zombies {
                older_than_minutes: 30,
            }),
        },
        TestCase {
            name: "A head that is only a prefix of a word is not a billing command",
            input: "!bai balanced diet tips",
            billing_access: BillingCommandAccess::Admin,
            expected: ControllerType::ChatCompletion(ChatCompletionControllerType::TextCommand),
        },
        TestCase {
            name: "A head without the command prefix is not a billing command",
            input: "balance",
            billing_access: BillingCommandAccess::Admin,
            expected: ControllerType::ChatCompletion(ChatCompletionControllerType::TextDirect),
        },
        TestCase {
            name: "Upstream commands are unaffected by billing access",
            input: "!bai usage",
            billing_access: BillingCommandAccess::Admin,
            expected: ControllerType::UsageHelp,
        },
    ];

    for test_case in test_cases {
        let result = super::determine_text_controller(
            command_prefix,
            test_case.input,
            super::TextGenerationPrefixRequirementType::No,
            false,
            test_case.billing_access,
            &open_gate(),
        );
        assert_eq!(result, test_case.expected, "Test case: {}", test_case.name);
    }
}

#[test]
fn commands_admin_only_gate() {
    use super::super::billing::{BillingCommandAccess, BillingControllerType};
    use super::ControllerType;
    use crate::controller;

    let command_prefix = "!bai";

    struct TestCase {
        name: &'static str,
        input: &'static str,
        sender_is_admin: bool,
        access: ConfigAccess,
        expected: ControllerType,
    }

    let test_cases = vec![
        TestCase {
            name: "Off: non-admin may use commands",
            input: "!bai help",
            sender_is_admin: false,
            access: access_config(false, &[]),
            expected: ControllerType::Help,
        },
        TestCase {
            name: "On: help from non-admin is ignored",
            input: "!bai help",
            sender_is_admin: false,
            access: access_config(true, &["balance", "topup", "image"]),
            expected: ControllerType::Ignore,
        },
        TestCase {
            name: "On: bare prefix (help) from non-admin is ignored",
            input: "!bai",
            sender_is_admin: false,
            access: access_config(true, &[]),
            expected: ControllerType::Ignore,
        },
        TestCase {
            name: "On: prefix-matched command from non-admin is ignored",
            input: "!bai helpme",
            sender_is_admin: false,
            access: access_config(true, &[]),
            expected: ControllerType::Ignore,
        },
        TestCase {
            name: "On: config from non-admin is ignored",
            input: "!bai config status",
            sender_is_admin: false,
            access: access_config(true, &[]),
            expected: ControllerType::Ignore,
        },
        TestCase {
            name: "On: unknown config arguments from non-admin are ignored",
            input: "!bai config bogus",
            sender_is_admin: false,
            access: access_config(true, &[]),
            expected: ControllerType::Ignore,
        },
        TestCase {
            name: "On: admin may use commands",
            input: "!bai config status",
            sender_is_admin: true,
            access: access_config(true, &[]),
            expected: ControllerType::Config(controller::cfg::ConfigControllerType::Status),
        },
        TestCase {
            name: "On: exempt command passes for non-admin",
            input: "!bai image create a cat",
            sender_is_admin: false,
            access: access_config(true, &["image"]),
            expected: ControllerType::ImageGeneration("a cat".to_owned()),
        },
        TestCase {
            name: "On: exempt command's usage help passes for non-admin",
            input: "!bai image",
            sender_is_admin: false,
            access: access_config(true, &["image"]),
            expected: ControllerType::UsageHelp,
        },
        TestCase {
            name: "On: exempt billing command passes for non-admin",
            input: "!bai balance",
            sender_is_admin: false,
            access: access_config(true, &["balance"]),
            expected: ControllerType::Billing(BillingControllerType::Balance),
        },
        TestCase {
            name: "On: non-exempt billing command from non-admin is ignored",
            input: "!bai stats day",
            sender_is_admin: false,
            access: access_config(true, &["balance"]),
            expected: ControllerType::Ignore,
        },
        TestCase {
            name: "On: exemption is by the first word",
            input: "!bai billing zombies",
            sender_is_admin: false,
            access: access_config(true, &["billing"]),
            expected: ControllerType::Billing(BillingControllerType::AccessDenied {
                command: "billing zombies",
            }),
        },
        TestCase {
            name: "On: conversation via the prefix is not a command",
            input: "!bai tell me a joke",
            sender_is_admin: false,
            access: access_config(true, &[]),
            expected: ControllerType::ChatCompletion(
                controller::chat_completion::ChatCompletionControllerType::TextCommand,
            ),
        },
        TestCase {
            name: "On: plain conversation is not a command",
            input: "hello",
            sender_is_admin: false,
            access: access_config(true, &[]),
            expected: ControllerType::ChatCompletion(
                controller::chat_completion::ChatCompletionControllerType::TextDirect,
            ),
        },
    ];

    for test_case in test_cases {
        let result = super::determine_text_controller(
            command_prefix,
            test_case.input,
            super::TextGenerationPrefixRequirementType::No,
            false,
            BillingCommandAccess::User,
            &CommandGate::new(test_case.sender_is_admin, &test_case.access),
        );
        assert_eq!(result, test_case.expected, "Test case: {}", test_case.name);
    }
}

#[test]
fn every_billing_head_is_a_known_command() {
    for head in crate::controller::billing::COMMAND_HEADS {
        assert!(super::COMMAND_HEADS.contains(head), "{head}");
    }
}
