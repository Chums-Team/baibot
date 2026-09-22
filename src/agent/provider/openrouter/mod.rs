use super::openai_compat::Config;

pub fn default_config() -> Config {
    let mut config = Config {
        base_url: "https://openrouter.ai/api/v1".to_owned(),

        text_to_speech: None,
        image_generation: None,
        speech_to_text: None,

        ..Default::default()
    };

    if let Some(ref mut config) = config.text_generation.as_mut() {
        config.model_id = "deepseek/deepseek-v4-flash-0731".to_owned();
        config.max_context_tokens = 128_000;
        config.max_response_tokens = Some(4096);
    }

    config
}
