mod prompt_variables;

pub use prompt_variables::TextGenerationPromptVariables;

#[derive(Default)]
pub struct TextGenerationParams {
    pub context_management_enabled: bool,
    pub prompt_override: Option<String>,
    pub temperature_override: Option<f32>,
    pub prompt_variables: TextGenerationPromptVariables,
}

/// Usage metadata reported by the provider for a single text generation.
///
/// Every field is optional: providers differ in what they report, and callers
/// (e.g. billing) must be able to cope with any of them being absent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextGenerationUsage {
    /// The authoritative total cost of the generation in USD, when the provider reports one.
    pub cost_usd: Option<f64>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
}

#[derive(Debug, Default)]
pub struct TextGenerationResult {
    pub text: String,
    /// Usage metadata, when the provider reports any. `None` means the provider does not
    /// expose usage information at all (as opposed to reporting zero usage).
    pub usage: Option<TextGenerationUsage>,
}

impl TextGenerationResult {
    /// A result carrying only the generated text and no usage metadata.
    pub fn text_only(text: String) -> Self {
        Self { text, usage: None }
    }
}
