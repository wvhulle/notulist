use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

use crate::error::ConfigError;
use crate::input_method::dictation_indicator_painter::parse_hex;
use crate::input_method::indicator_phase::IndicatorPhase;

pub type Rgba = [u8; 4];

const READY_FOREGROUND: Rgba = [0xd4, 0xd4, 0xd8, 0xff];
const DIM_GREY: Rgba = [0x6b, 0x72, 0x80, 0xff];
const WAITING_GREY: Rgba = [0x9c, 0xa3, 0xaf, 0xff];
const LOADING_SLATE: Rgba = [0x94, 0xa3, 0xb8, 0xff];
const FAILED_RED: Rgba = [0xef, 0x44, 0x44, 0xff];
const HEARING_BLUE: Rgba = [0x3b, 0x82, 0xf6, 0xff];
const AMBER: Rgba = [0xf5, 0x9e, 0x0b, 0xff];
const CORRECTING_VIOLET: Rgba = [0xa8, 0x55, 0xf7, 0xff];
const COMMITTED_GREEN: Rgba = [0x22, 0xc5, 0x5e, 0xff];
const LEVEL_FRAME_MS: u64 = 10;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ImeConfigFile {
    pub ime: ImeConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ImeConfig {
    pub typing: TypingConfig,
    pub popup: PopupConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TypingConfig {
    pub fall_back_to_dotool: bool,
    pub wait_for_modifier_release_ms: u64,
}

impl Default for TypingConfig {
    fn default() -> Self {
        Self {
            fall_back_to_dotool: true,
            wait_for_modifier_release_ms: 2000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PopupConfig {
    pub show_popup: bool,
    pub animations_enabled: bool,
    pub pill_height_px: u32,
    pub inner_padding_px: u32,
    pub corner_radius_px: u32,
    pub spacing_between_elements_px: u32,
    #[serde(deserialize_with = "deserialize_colour")]
    pub background_colour: Rgba,
    pub font_file: Option<PathBuf>,
    pub mic_button: MicButtonConfig,
    pub level_meter: LevelMeterConfig,
    pub phase_label: PhaseLabelConfig,
    pub speech_detection: SpeechDetectionConfig,
    pub flashes: FlashConfig,
    pub phase_styles: PhaseStylesConfig,
}

impl Default for PopupConfig {
    fn default() -> Self {
        Self {
            show_popup: true,
            animations_enabled: true,
            pill_height_px: 22,
            inner_padding_px: 4,
            corner_radius_px: 5,
            spacing_between_elements_px: 4,
            background_colour: [0x1e, 0x1e, 0x22, 0xb3],
            font_file: None,
            mic_button: MicButtonConfig::default(),
            level_meter: LevelMeterConfig::default(),
            phase_label: PhaseLabelConfig::default(),
            speech_detection: SpeechDetectionConfig::default(),
            flashes: FlashConfig::default(),
            phase_styles: PhaseStylesConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MicButtonConfig {
    pub show_mic_button: bool,
    pub mic_button_size_px: u32,
}

impl Default for MicButtonConfig {
    fn default() -> Self {
        Self {
            show_mic_button: true,
            mic_button_size_px: 14,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LevelMeterConfig {
    pub show_level_meter: bool,
    pub bar_count: usize,
    pub bar_width_px: u32,
    pub spacing_between_bars_px: u32,
    pub visible_history_ms: u64,
    pub silence_floor_dbfs: f64,
}

impl Default for LevelMeterConfig {
    fn default() -> Self {
        Self {
            show_level_meter: true,
            bar_count: 12,
            bar_width_px: 2,
            spacing_between_bars_px: 1,
            visible_history_ms: 600,
            silence_floor_dbfs: -60.0,
        }
    }
}

impl LevelMeterConfig {
    pub fn frames_per_bar(&self) -> usize {
        let frames_per_history = self.visible_history_ms / LEVEL_FRAME_MS;
        let per_bar = frames_per_history / self.bar_count.max(1) as u64;
        usize::try_from(per_bar.max(1)).unwrap_or(1)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PhaseLabelConfig {
    pub show_phase_label: bool,
    pub font_size_px: u32,
}

impl Default for PhaseLabelConfig {
    fn default() -> Self {
        Self {
            show_phase_label: true,
            font_size_px: 11,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SpeechDetectionConfig {
    pub speech_loudness_threshold_dbfs: f64,
    pub keep_hearing_after_speech_ms: u64,
}

impl Default for SpeechDetectionConfig {
    fn default() -> Self {
        Self {
            speech_loudness_threshold_dbfs: -40.0,
            keep_hearing_after_speech_ms: 300,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FlashConfig {
    pub correction_flash_ms: u64,
    pub session_outcome_flash_ms: u64,
}

impl Default for FlashConfig {
    fn default() -> Self {
        Self {
            correction_flash_ms: 400,
            session_outcome_flash_ms: 800,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PhaseStylesConfig {
    pub daemon_offline: PhaseStyleConfig,
    pub loading_model: PhaseStyleConfig,
    pub ready: PhaseStyleConfig,
    pub failed: PhaseStyleConfig,
    pub waiting_for_speech: PhaseStyleConfig,
    pub hearing: PhaseStyleConfig,
    pub decoding: PhaseStyleConfig,
    pub correcting: PhaseStyleConfig,
    pub finishing: PhaseStyleConfig,
    pub committed: PhaseStyleConfig,
    pub cancelled: PhaseStyleConfig,
    pub password_field: PhaseStyleConfig,
}

impl PhaseStylesConfig {
    pub fn style_for(&self, phase: IndicatorPhase) -> PhaseStyle {
        match phase {
            IndicatorPhase::Hidden | IndicatorPhase::Ready => {
                self.ready.resolve("", READY_FOREGROUND)
            }
            IndicatorPhase::Offline => self.daemon_offline.resolve("Offline", DIM_GREY),
            IndicatorPhase::LoadingModel => self.loading_model.resolve("Loading", LOADING_SLATE),
            IndicatorPhase::Failed => self.failed.resolve("Failed", FAILED_RED),
            IndicatorPhase::Waiting => self.waiting_for_speech.resolve("Waiting", WAITING_GREY),
            IndicatorPhase::Hearing => self.hearing.resolve("Hearing", HEARING_BLUE),
            IndicatorPhase::Decoding => self.decoding.resolve("Decoding", AMBER),
            IndicatorPhase::Correcting => self.correcting.resolve("Correcting", CORRECTING_VIOLET),
            IndicatorPhase::Finishing => self.finishing.resolve("Finishing", AMBER),
            IndicatorPhase::Committed => self.committed.resolve("Done", COMMITTED_GREEN),
            IndicatorPhase::Cancelled => self.cancelled.resolve("Cancelled", DIM_GREY),
            IndicatorPhase::Password => self.password_field.resolve("Password", DIM_GREY),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PhaseStyleConfig {
    pub label_text: Option<String>,
    #[serde(deserialize_with = "deserialize_optional_colour")]
    pub colour: Option<Rgba>,
}

impl PhaseStyleConfig {
    fn resolve(&self, default_label: &str, default_colour: Rgba) -> PhaseStyle {
        PhaseStyle {
            label_text: self
                .label_text
                .clone()
                .unwrap_or_else(|| default_label.to_owned()),
            colour: self.colour.unwrap_or(default_colour),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseStyle {
    pub label_text: String,
    pub colour: Rgba,
}

impl ImeConfig {
    pub fn load(explicit_path: Option<PathBuf>) -> Result<Self, ConfigError> {
        if let Some(path) = explicit_path {
            if !path.exists() {
                return Err(ConfigError::ExplicitFileMissing);
            }
            return read_config_file(&path);
        }
        match default_config_path() {
            Some(path) if path.exists() => read_config_file(&path),
            _ => Ok(Self::default()),
        }
    }
}

fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("notulist").join("config.toml"))
}

fn read_config_file(path: &Path) -> Result<ImeConfig, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(ConfigError::Read)?;
    parse_config(&text).map_err(ConfigError::Parse)
}

fn parse_config(text: &str) -> Result<ImeConfig, toml::de::Error> {
    let file: ImeConfigFile = toml::from_str(text)?;
    Ok(file.ime)
}

fn deserialize_colour<'de, D>(deserializer: D) -> Result<Rgba, D::Error>
where
    D: Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    let Some(colour) = parse_hex(&text) else {
        return Err(serde::de::Error::custom(
            "colour must be #RRGGBB or #RRGGBBAA",
        ));
    };
    Ok(colour)
}

fn deserialize_optional_colour<'de, D>(deserializer: D) -> Result<Option<Rgba>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(text) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let Some(colour) = parse_hex(&text) else {
        return Err(serde::de::Error::custom(
            "colour must be #RRGGBB or #RRGGBBAA",
        ));
    };
    Ok(Some(colour))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_yields_the_defaults() {
        let config = parse_config("").expect("parses");
        assert!(config.typing.fall_back_to_dotool);
        assert_eq!(config.typing.wait_for_modifier_release_ms, 2000);
        assert_eq!(config.popup.pill_height_px, 22);
        assert_eq!(config.popup.level_meter.bar_count, 12);
    }

    #[test]
    fn daemon_tables_are_ignored() {
        let config = parse_config(
            r#"
            [daemon.whisper]
            model = "base.en"

            [daemon.microphone]
            device_name = "default"
            "#,
        )
        .expect("parses");
        assert_eq!(config.popup.pill_height_px, 22);
    }

    #[test]
    fn popup_tables_override_the_defaults() {
        let config = parse_config(
            r##"
            [ime.typing]
            fall_back_to_dotool = false
            wait_for_modifier_release_ms = 500

            [ime.popup]
            pill_height_px = 30
            background_colour = "#00000080"

            [ime.popup.mic_button]
            show_mic_button = false

            [ime.popup.level_meter]
            bar_count = 8
            visible_history_ms = 800
            "##,
        )
        .expect("parses");
        assert!(!config.typing.fall_back_to_dotool);
        assert_eq!(config.typing.wait_for_modifier_release_ms, 500);
        assert_eq!(config.popup.pill_height_px, 30);
        assert_eq!(config.popup.background_colour, [0, 0, 0, 0x80]);
        assert!(!config.popup.mic_button.show_mic_button);
        assert_eq!(config.popup.mic_button.mic_button_size_px, 14);
        assert_eq!(config.popup.level_meter.bar_count, 8);
    }

    #[test]
    fn phase_style_overrides_keep_the_default_for_missing_keys() {
        let config = parse_config(
            r##"
            [ime.popup.phase_styles.hearing]
            label_text = "Listening"
            "##,
        )
        .expect("parses");
        let style = config.popup.phase_styles.style_for(IndicatorPhase::Hearing);
        assert_eq!(style.label_text, "Listening");
        assert_eq!(style.colour, HEARING_BLUE);
    }

    #[test]
    fn phase_style_colour_overrides_the_default_colour() {
        let config = parse_config(
            r##"
            [ime.popup.phase_styles.failed]
            colour = "#ff00ff"
            "##,
        )
        .expect("parses");
        let style = config.popup.phase_styles.style_for(IndicatorPhase::Failed);
        assert_eq!(style.label_text, "Failed");
        assert_eq!(style.colour, [0xff, 0x00, 0xff, 0xff]);
    }

    #[test]
    fn malformed_colours_are_rejected() {
        let result = parse_config(
            r##"
            [ime.popup]
            background_colour = "blue"
            "##,
        );
        assert!(result.is_err());
    }

    #[test]
    fn frames_per_bar_spreads_the_history_over_the_bars() {
        let meter = LevelMeterConfig::default();
        assert_eq!(meter.frames_per_bar(), 5);
        let short = LevelMeterConfig {
            visible_history_ms: 1,
            ..LevelMeterConfig::default()
        };
        assert_eq!(short.frames_per_bar(), 1);
    }

    #[test]
    fn every_active_phase_has_a_label_and_idle_has_none() {
        let styles = PhaseStylesConfig::default();
        assert!(IndicatorPhase::ALL
            .iter()
            .filter(|phase| !phase.is_idle())
            .all(|phase| !styles.style_for(*phase).label_text.is_empty()));
        assert!(styles
            .style_for(IndicatorPhase::Ready)
            .label_text
            .is_empty());
        assert_eq!(
            styles.style_for(IndicatorPhase::Ready).colour,
            READY_FOREGROUND
        );
    }

    #[test]
    fn default_background_is_translucent() {
        assert!(PopupConfig::default().background_colour[3] < 0xff);
    }
}
