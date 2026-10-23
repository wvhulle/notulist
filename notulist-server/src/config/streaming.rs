use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default)]
pub struct StreamingConfig {
    pub retranscribe_interval_secs: f32,
    pub max_window_secs: f32,
    pub min_window_rms_for_speech: f32,
    pub min_audio_before_first_pass_secs: f32,
    pub language_detection_audio_secs: f32,
    pub min_new_words_per_partial: usize,
    pub type_partials_live: bool,
    pub revise_recent_words: bool,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            retranscribe_interval_secs: 0.8,
            max_window_secs: 29.0,
            min_window_rms_for_speech: 0.005,
            min_audio_before_first_pass_secs: 1.0,
            language_detection_audio_secs: 2.0,
            min_new_words_per_partial: 1,
            type_partials_live: true,
            revise_recent_words: false,
        }
    }
}
