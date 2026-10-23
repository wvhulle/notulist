mod language;
mod microphone;
mod shortcut;
mod streaming;
mod text_cleanup;
mod whisper;

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::ConfigError;

pub use language::LanguageConfig;
pub use microphone::MicrophoneConfig;
pub use shortcut::{ShortcutChords, ShortcutConfig};
pub use streaming::StreamingConfig;
pub use text_cleanup::{default_filler_words, TextCleanupConfig};
pub use whisper::WhisperConfig;

const PROJECT_DIRECTORY: &str = "notulist";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub microphone: MicrophoneConfig,
    pub whisper: WhisperConfig,
    pub streaming: StreamingConfig,
    pub text_cleanup: TextCleanupConfig,
    pub shortcut: ShortcutConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigFile {
    daemon: DaemonConfig,
}

const WHISPER_MODEL_ENVIRONMENT_VARIABLE: &str = "NOTULIST_WHISPER_MODEL";
const WHISPER_LANGUAGES_ENVIRONMENT_VARIABLE: &str = "NOTULIST_WHISPER_LANGUAGES";

impl DaemonConfig {
    pub fn load(explicit_path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut config = Self::load_file(explicit_path)?;
        if let Ok(model) = std::env::var(WHISPER_MODEL_ENVIRONMENT_VARIABLE) {
            tracing::info!(
                model,
                "Using the whisper model from {WHISPER_MODEL_ENVIRONMENT_VARIABLE}"
            );
            config.whisper.model = model;
        }
        if let Ok(language_list) = std::env::var(WHISPER_LANGUAGES_ENVIRONMENT_VARIABLE) {
            if let Some(languages) = parse_comma_separated_languages(&language_list) {
                tracing::info!(
                    ?languages,
                    "Using the whisper languages from {WHISPER_LANGUAGES_ENVIRONMENT_VARIABLE}"
                );
                config.whisper.languages = languages;
            }
        }
        Ok(config)
    }

    fn load_file(explicit_path: Option<&Path>) -> Result<Self, ConfigError> {
        let Some(path) = explicit_path
            .map(Path::to_path_buf)
            .or_else(default_config_path)
        else {
            return Ok(Self::default());
        };
        if !path.exists() {
            if explicit_path.is_some() {
                tracing::error!(path = %path.display(), "config file does not exist");
                return Err(ConfigError::ExplicitFileMissing);
            }
            hint_at_legacy_voxtype_config(&path);
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(&path).inspect_err(
            |_| tracing::error!(path = %path.display(), "could not read the config file"),
        )?;
        let file: ConfigFile = toml::from_str(&contents).inspect_err(
            |_| tracing::error!(path = %path.display(), "could not parse the config file"),
        )?;
        Ok(file.daemon)
    }
}

fn parse_comma_separated_languages(language_list: &str) -> Option<LanguageConfig> {
    let mut language_codes: Vec<String> = language_list
        .split(',')
        .map(str::trim)
        .filter(|language_code| !language_code.is_empty())
        .map(str::to_owned)
        .collect();
    match language_codes.len() {
        0 => None,
        1 => language_codes.pop().map(LanguageConfig::Single),
        _ => Some(LanguageConfig::Multiple(language_codes)),
    }
}

pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(PROJECT_DIRECTORY).join("config.toml"))
}

pub fn models_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(PROJECT_DIRECTORY)
        .join("models")
}

fn hint_at_legacy_voxtype_config(missing_path: &Path) {
    let Some(legacy_path) = dirs::config_dir().map(|dir| dir.join("voxtype").join("config.toml"))
    else {
        return;
    };
    if legacy_path.exists() {
        tracing::warn!(
            "No config at {}. A voxtype config exists at {}; move its settings into the [daemon] table using the new names: [daemon.microphone], [daemon.whisper], [daemon.streaming] and [daemon.text_cleanup].",
            missing_path.display(),
            legacy_path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comma_separated_languages_become_single_or_multiple() {
        assert_eq!(
            parse_comma_separated_languages("nl"),
            Some(LanguageConfig::Single("nl".to_string()))
        );
        assert_eq!(
            parse_comma_separated_languages(" en, nl ,"),
            Some(LanguageConfig::Multiple(vec![
                "en".to_string(),
                "nl".to_string()
            ]))
        );
        assert_eq!(parse_comma_separated_languages(" , "), None);
    }

    #[test]
    fn daemon_table_parses_with_descriptive_keys() {
        let file: ConfigFile = toml::from_str(
            r#"
            [daemon.whisper]
            model = "/models/ggml-large-v3-turbo.bin"
            languages = ["en", "nl"]

            [daemon.streaming]
            retranscribe_interval_secs = 0.6
            revise_recent_words = true
            "#,
        )
        .expect("parses");
        let config = file.daemon;
        assert_eq!(config.whisper.model, "/models/ggml-large-v3-turbo.bin");
        assert!(config.whisper.languages.is_multiple());
        assert_eq!(config.streaming.retranscribe_interval_secs, 0.6);
        assert!(config.streaming.revise_recent_words);
        assert_eq!(config.streaming.max_window_secs, 29.0);
        assert_eq!(config.microphone.capture_sample_rate_hz, 16000);
    }

    #[test]
    fn other_tables_are_ignored() {
        let file: ConfigFile = toml::from_str(
            r#"
            [ime.popup]
            show_popup = true

            [daemon.microphone]
            device_name = "usb"
            "#,
        )
        .expect("parses");
        assert_eq!(file.daemon.microphone.device_name, "usb");
    }

    #[test]
    fn empty_config_uses_defaults() {
        let file: ConfigFile = toml::from_str("").expect("parses");
        assert_eq!(file.daemon.whisper.model, "base.en");
        assert!(file.daemon.streaming.type_partials_live);
        assert!(file.daemon.shortcut.enabled);
        assert_eq!(file.daemon.shortcut.hold_keys, vec!["LEFTMETA", "V"]);
        assert_eq!(file.daemon.shortcut.toggle_keys, vec!["LEFTMETA", "T"]);
        assert_eq!(
            file.daemon.shortcut.cancel_keys,
            vec!["LEFTMETA", "LEFTSHIFT", "V"]
        );
    }

    #[test]
    fn shortcut_table_parses() {
        let file: ConfigFile = toml::from_str(
            r#"
            [daemon.shortcut]
            enabled = false
            hold_keys = []
            toggle_keys = ["LEFTCTRL", "SPACE"]
            cancel_keys = []
            "#,
        )
        .expect("parses");
        let shortcut = file.daemon.shortcut;
        assert!(!shortcut.enabled);
        assert!(shortcut.hold_keys.is_empty());
        assert_eq!(shortcut.toggle_keys, vec!["LEFTCTRL", "SPACE"]);
        assert!(shortcut.cancel_keys.is_empty());
    }
}
