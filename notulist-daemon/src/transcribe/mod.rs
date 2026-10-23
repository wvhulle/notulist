pub mod sliding_window;
pub mod streaming;
pub mod whisper;
mod whisper_model_context;
mod whisper_model_files;

use crate::error::TranscribeError;

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, samples: &[f32]) -> Result<String, TranscribeError>;
}
