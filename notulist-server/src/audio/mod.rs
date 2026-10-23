pub mod cpal_capture;
pub mod level_bucketer;
pub mod resampler;
pub mod wav_playback;

use std::path::PathBuf;

use tokio::sync::{mpsc, oneshot};

use crate::config::MicrophoneConfig;
use crate::error::{CaptureError, MicrophoneError};

const WHISPER_SAMPLE_RATE_HZ: u32 = 16_000;

pub struct CapturedAudio {
    pub chunks: mpsc::Receiver<Vec<f32>>,
    pub stop: StopCapture,
    pub disconnect: oneshot::Receiver<MicrophoneError>,
}

pub struct StopCapture {
    signal: oneshot::Sender<()>,
}

impl StopCapture {
    pub fn new(signal: oneshot::Sender<()>) -> Self {
        Self { signal }
    }

    pub fn stop(self) {
        let _ = self.signal.send(());
    }
}

#[derive(Debug, Clone)]
pub enum AudioSource {
    Microphone(MicrophoneConfig),
    WavFile { path: PathBuf, playback_speed: f32 },
}

impl AudioSource {
    pub fn sample_rate_hz(&self) -> u32 {
        match self {
            AudioSource::Microphone(config) => config.capture_sample_rate_hz,
            AudioSource::WavFile { .. } => WHISPER_SAMPLE_RATE_HZ,
        }
    }
}

pub fn start_capture(source: &AudioSource) -> Result<CapturedAudio, CaptureError> {
    match source {
        AudioSource::Microphone(config) => Ok(cpal_capture::start_microphone(config)?),
        AudioSource::WavFile {
            path,
            playback_speed,
        } => wav_playback::start_wav_playback(path, *playback_speed),
    }
}
