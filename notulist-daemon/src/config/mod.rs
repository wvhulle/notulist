mod language;
mod microphone;
mod streaming;
mod text_cleanup;
mod whisper;

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::ConfigError;

pub use language::LanguageConfig;
pub use microphone::MicrophoneConfig;
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
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigFile {
    daemon: DaemonConfig,
}

impl DaemonConfig {
    pub fn load(explicit_path: Option<&Path>) -> Result<Self, ConfigError> {
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
    }
}
