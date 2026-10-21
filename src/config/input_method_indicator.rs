//! `[input_method_indicator]` configuration block.

use serde::{Deserialize, Serialize};

/// On-screen indicator drawn at the focused text field through the
/// input-method v2 popup surface.
///
/// Colours are `#RRGGBB` hex strings. Unset fields fall back to the built-in
/// palette, so the table can be omitted entirely.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct InputMethodIndicatorConfig {
    /// Show the indicator pill on every focused text field.
    pub enabled: bool,

    /// Freeze the waveform and spinner; only repaint on state transitions.
    pub reduced_motion: bool,

    /// Font file to use for the label. When unset, resolved once with
    /// `fc-match -f '%{file}' sans`.
    pub font: Option<String>,

    /// Pill background colour.
    pub background: Option<String>,

    /// Label and idle-dot colour.
    pub foreground: Option<String>,

    /// Listening waveform colour.
    pub accent: Option<String>,

    /// Processing spinner colour.
    pub warning: Option<String>,
}

impl Default for InputMethodIndicatorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reduced_motion: false,
            font: None,
            background: None,
            foreground: None,
            accent: None,
            warning: None,
        }
    }
}