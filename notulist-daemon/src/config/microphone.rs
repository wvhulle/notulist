use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MicrophoneConfig {
    pub device_name: String,
    pub capture_sample_rate_hz: u32,
    pub max_session_secs: u32,
}

impl Default for MicrophoneConfig {
    fn default() -> Self {
        Self {
            device_name: "default".to_string(),
            capture_sample_rate_hz: 16000,
            max_session_secs: 300,
        }
    }
}
