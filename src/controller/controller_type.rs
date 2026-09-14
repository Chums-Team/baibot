#[derive(Debug, PartialEq)]
pub enum ControllerType {
    // Denotes that the message is to be ignored.
    Ignore,

    Help,

    UsageHelp,

    Unknown,

    Error(String),
    ErrorInThread(String, mxlink::ThreadInfo),

    ProviderHelp,

    Access(super::access::AccessControllerType),

    Agent(super::agent::AgentControllerType),

    Config(super::cfg::ConfigControllerType),

    Billing(super::billing::BillingControllerType),

    ChatCompletion(super::chat_completion::ChatCompletionControllerType),

    ImageGeneration(String),
    ImageEdit(String),
    StickerGeneration(String),
}
