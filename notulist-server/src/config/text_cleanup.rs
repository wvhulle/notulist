use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TextCleanupConfig {
    pub spoken_punctuation: bool,
    pub word_replacements: HashMap<String, String>,
    pub remove_filler_words: bool,
    pub filler_words: Vec<String>,
}

impl Default for TextCleanupConfig {
    fn default() -> Self {
        Self {
            spoken_punctuation: false,
            word_replacements: HashMap::new(),
            remove_filler_words: true,
            filler_words: default_filler_words(),
        }
    }
}

pub fn default_filler_words() -> Vec<String> {
    vec![
        "uh".to_string(),
        "um".to_string(),
        "er".to_string(),
        "ah".to_string(),
        "eh".to_string(),
        "hmm".to_string(),
        "hm".to_string(),
        "mm".to_string(),
        "mhm".to_string(),
    ]
}
