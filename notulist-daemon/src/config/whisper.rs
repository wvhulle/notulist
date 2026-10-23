use serde::Deserialize;

use super::LanguageConfig;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WhisperConfig {
    pub model: String,
    pub languages: LanguageConfig,
    pub translate_to_english: bool,
    pub inference_threads: Option<usize>,
    pub gpu_device_index: Option<i32>,
    pub flash_attention: bool,
    pub shrink_context_for_short_audio: bool,
    pub initial_prompt: Option<String>,
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            model: "base.en".to_string(),
            languages: LanguageConfig::default(),
            translate_to_english: false,
            inference_threads: None,
            gpu_device_index: None,
            flash_attention: false,
            shrink_context_for_short_audio: false,
            initial_prompt: None,
        }
    }
}
