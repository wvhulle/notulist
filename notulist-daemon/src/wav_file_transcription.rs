use std::path::Path;

use crate::audio::wav_playback::read_wav_as_whisper_input;
use crate::config::DaemonConfig;
use crate::error::DaemonError;
use crate::spoken_text_processor::TextProcessor;
use crate::transcribe::whisper::WhisperTranscriber;
use crate::transcribe::Transcriber;

pub fn transcribe_wav_file(config: &DaemonConfig, path: &Path) -> Result<String, DaemonError> {
    let samples = read_wav_as_whisper_input(path)?;
    let whisper = WhisperTranscriber::new(&config.whisper)?;
    let text = whisper.transcribe(&samples)?;
    Ok(TextProcessor::new(&config.text_cleanup).process(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_gets_replacements_and_spoken_punctuation() {
        let mut config = DaemonConfig::default();
        config
            .text_cleanup
            .word_replacements
            .insert("vox type".to_string(), "notulist".to_string());
        config.text_cleanup.spoken_punctuation = true;
        assert_eq!(
            TextProcessor::new(&config.text_cleanup).process("I use vox type period"),
            "I use notulist."
        );
    }
}
