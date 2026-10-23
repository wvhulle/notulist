use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum LanguageConfig {
    Single(String),
    Multiple(Vec<String>),
}

impl Default for LanguageConfig {
    fn default() -> Self {
        LanguageConfig::Single("en".to_string())
    }
}

impl LanguageConfig {
    pub fn as_vec(&self) -> Vec<String> {
        match self {
            LanguageConfig::Single(s) => vec![s.clone()],
            LanguageConfig::Multiple(v) => v.clone(),
        }
    }

    pub fn is_auto(&self) -> bool {
        matches!(self, LanguageConfig::Single(s) if s == "auto")
    }

    pub fn is_multiple(&self) -> bool {
        matches!(self, LanguageConfig::Multiple(v) if v.len() > 1)
    }

    pub fn primary(&self) -> &str {
        match self {
            LanguageConfig::Single(s) => s,
            LanguageConfig::Multiple(v) => v.first().map(|s| s.as_str()).unwrap_or("en"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WhisperConfig;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct TestConfig {
        whisper: WhisperConfig,
    }

    fn parse_languages(languages_toml: &str) -> LanguageConfig {
        let config: TestConfig =
            toml::from_str(&format!("[whisper]\nlanguages = {languages_toml}")).expect("parses");
        config.whisper.languages
    }

    #[test]
    fn single_language_is_neither_auto_nor_multiple() {
        let languages = parse_languages("\"fr\"");
        assert_eq!(languages, LanguageConfig::Single("fr".to_string()));
        assert!(!languages.is_auto());
        assert!(!languages.is_multiple());
        assert_eq!(languages.primary(), "fr");
        assert_eq!(languages.as_vec(), vec!["fr"]);
    }

    #[test]
    fn auto_is_unconstrained_detection() {
        let languages = parse_languages("\"auto\"");
        assert!(languages.is_auto());
        assert!(!languages.is_multiple());
        assert_eq!(languages.primary(), "auto");
    }

    #[test]
    fn array_is_constrained_detection() {
        let languages = parse_languages("[\"en\", \"fr\", \"de\"]");
        assert_eq!(
            languages,
            LanguageConfig::Multiple(vec!["en".to_string(), "fr".to_string(), "de".to_string()])
        );
        assert!(!languages.is_auto());
        assert!(languages.is_multiple());
        assert_eq!(languages.primary(), "en");
        assert_eq!(languages.as_vec(), vec!["en", "fr", "de"]);
    }

    #[test]
    fn single_element_array_is_not_multiple() {
        let languages = parse_languages("[\"en\"]");
        assert!(!languages.is_multiple());
        assert_eq!(languages.primary(), "en");
    }

    #[test]
    fn default_is_english() {
        let languages = LanguageConfig::default();
        assert_eq!(languages, LanguageConfig::Single("en".to_string()));
        assert!(!languages.is_auto());
        assert!(!languages.is_multiple());
        assert_eq!(languages.primary(), "en");
    }
}
