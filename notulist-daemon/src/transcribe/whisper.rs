use std::path::PathBuf;

use super::whisper_model_context::{
    language_code_for_id, language_id_for_code, ContextOptions, DecodingRequest, DecodingState,
    WhisperModelContext,
};
use super::Transcriber;
use crate::config::{models_dir, LanguageConfig, WhisperConfig};
use crate::error::TranscribeError;

const SAMPLE_RATE_HZ: f32 = 16_000.0;

pub struct WhisperTranscriber {
    model_context: WhisperModelContext,
    languages: LanguageConfig,
    translate_to_english: bool,
    inference_threads: i32,
    shrink_context_for_short_audio: bool,
    initial_prompt: Option<String>,
}

struct LanguageChoice {
    language_code: String,
    probability: f32,
}

impl WhisperTranscriber {
    pub fn new(config: &WhisperConfig) -> Result<Self, TranscribeError> {
        let model_path = resolve_model_path(&config.model)?;

        tracing::info!("Loading whisper model from {:?}", model_path);
        let start = std::time::Instant::now();
        if let Some(device) = config.gpu_device_index {
            tracing::info!("Using GPU device index {}", device);
        }
        if config.flash_attention {
            tracing::info!("Flash attention enabled");
        }
        let model_context = WhisperModelContext::load(
            &model_path,
            &ContextOptions {
                gpu_device_index: config.gpu_device_index,
                flash_attention: config.flash_attention,
            },
        )?;
        tracing::info!("Model loaded in {:.2}s", start.elapsed().as_secs_f32());

        let inference_threads = config
            .inference_threads
            .unwrap_or_else(|| num_cpus::get().min(4));

        Ok(Self {
            model_context,
            languages: config.languages.clone(),
            translate_to_english: config.translate_to_english,
            inference_threads: i32::try_from(inference_threads)?,
            shrink_context_for_short_audio: config.shrink_context_for_short_audio,
            initial_prompt: config.initial_prompt.clone(),
        })
    }

    fn decoding_request<'request>(
        &'request self,
        selected_language: Option<&'request str>,
        duration_secs: f32,
        retry: bool,
    ) -> DecodingRequest<'request> {
        let audio_context_size = (self.shrink_context_for_short_audio && !retry)
            .then(|| calculate_audio_ctx(duration_secs))
            .flatten();
        if let Some(audio_context_size) = audio_context_size {
            tracing::info!(
                "Audio context optimization: using audio_ctx={} for {:.2}s clip",
                audio_context_size,
                duration_secs
            );
        }
        DecodingRequest {
            language: selected_language,
            translate_to_english: self.translate_to_english,
            inference_threads: self.inference_threads,
            initial_prompt: self.initial_prompt.as_deref(),
            single_segment: duration_secs < 30.0 && !retry,
            audio_context_size,
            use_beam_search: retry,
        }
    }

    fn run_full(
        &self,
        samples: &[f32],
        selected_language: Option<&str>,
        duration_secs: f32,
        retry: bool,
    ) -> Result<String, TranscribeError> {
        let mut state = self.model_context.new_decoding_state()?;
        let request = self.decoding_request(selected_language, duration_secs, retry);
        state.decode(samples, &request)
    }

    fn select_language_from_allowed(
        &self,
        state: &mut DecodingState<'_>,
        samples: &[f32],
        allowed: &[String],
    ) -> Result<String, TranscribeError> {
        let detection = state.detect_language(samples, self.inference_threads)?;

        let best = allowed
            .iter()
            .filter_map(|language_code| {
                let Some(language_id) = language_id_for_code(language_code) else {
                    tracing::warn!(
                        "Unknown language code '{}' in language array",
                        language_code
                    );
                    return None;
                };
                detection
                    .probabilities
                    .get(language_id)
                    .map(|&probability| LanguageChoice {
                        language_code: language_code.clone(),
                        probability,
                    })
            })
            .max_by(|left, right| left.probability.total_cmp(&right.probability));

        let choice = best.unwrap_or_else(|| {
            let fallback = allowed.first().cloned().unwrap_or_else(|| "en".to_string());
            tracing::warn!(
                "No valid languages found in allowed set {:?}, using first: {}",
                allowed,
                fallback
            );
            LanguageChoice {
                language_code: fallback,
                probability: -1.0,
            }
        });

        let detected_language =
            language_code_for_id(detection.detected_language_id).unwrap_or("unknown");
        tracing::info!(
            "Language detection: Whisper detected '{}', selected '{}' (prob={:.1}%) from allowed {:?}",
            detected_language,
            choice.language_code,
            choice.probability * 100.0,
            allowed
        );

        Ok(choice.language_code)
    }
}

impl Transcriber for WhisperTranscriber {
    fn transcribe(&self, samples: &[f32]) -> Result<String, TranscribeError> {
        if samples.is_empty() {
            tracing::debug!("Skipping whisper inference for an empty audio buffer");
            return Ok(String::new());
        }

        let duration_secs = samples.len() as f32 / SAMPLE_RATE_HZ;
        tracing::debug!(
            "Transcribing {:.2}s of audio ({} samples)",
            duration_secs,
            samples.len()
        );

        let start = std::time::Instant::now();

        let mut state = self.model_context.new_decoding_state()?;

        let selected_language: Option<String> = if self.languages.is_auto() {
            tracing::debug!("Using unconstrained language auto-detection");
            None
        } else if self.languages.is_multiple() {
            let allowed = self.languages.as_vec();
            tracing::debug!("Using constrained language detection from: {:?}", allowed);
            Some(self.select_language_from_allowed(&mut state, samples, &allowed)?)
        } else {
            let lang = self.languages.primary().to_string();
            tracing::debug!("Using specified language: {}", lang);
            Some(lang)
        };

        let mut result =
            self.run_full(samples, selected_language.as_deref(), duration_secs, false)?;

        if duration_secs >= 1.0 && is_degenerate_transcript(&result) {
            tracing::warn!(
                "Whisper returned degenerate transcript {:?} for {:.2}s audio; retrying with beam search",
                result,
                duration_secs
            );
            let retry_result =
                self.run_full(samples, selected_language.as_deref(), duration_secs, true)?;
            if is_degenerate_transcript(&retry_result) {
                tracing::warn!(
                    "Whisper retry also returned degenerate transcript {:?}; treating as empty",
                    retry_result
                );
                result.clear();
            } else {
                tracing::info!("Whisper retry recovered transcript");
                result = retry_result;
            }
        }

        tracing::info!(
            "Transcription completed in {:.2}s: {:?}",
            start.elapsed().as_secs_f32(),
            if result.chars().count() > 50 {
                format!("{}...", result.chars().take(50).collect::<String>())
            } else {
                result.clone()
            }
        );

        Ok(result)
    }
}

fn is_degenerate_transcript(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }

    !trimmed.chars().any(|c| c.is_alphanumeric())
}

fn resolve_model_path(model: &str) -> Result<PathBuf, TranscribeError> {
    let path = PathBuf::from(model);
    if path.is_absolute() && path.exists() {
        return Ok(path);
    }

    let model_filename = match model {
        "tiny" => "ggml-tiny.bin",
        "tiny.en" => "ggml-tiny.en.bin",
        "base" => "ggml-base.bin",
        "base.en" => "ggml-base.en.bin",
        "small" => "ggml-small.bin",
        "small.en" => "ggml-small.en.bin",
        "medium" => "ggml-medium.bin",
        "medium.en" => "ggml-medium.en.bin",
        "large" | "large-v1" => "ggml-large-v1.bin",
        "large-v2" => "ggml-large-v2.bin",
        "large-v3" => "ggml-large-v3.bin",
        "large-v3-turbo" => "ggml-large-v3-turbo.bin",
        other if other.ends_with(".bin") => other,
        other => {
            tracing::error!(
                model = other,
                "Unknown model name; valid names: tiny, base, small, medium, large-v3, large-v3-turbo"
            );
            return Err(TranscribeError::UnknownModelName);
        }
    };

    let model_path = models_dir().join(model_filename);
    let cwd_path = PathBuf::from(model_filename);
    let local_models_path = PathBuf::from("models").join(model_filename);
    let candidates = [model_path, cwd_path, local_models_path];

    match candidates.into_iter().find(|candidate| candidate.exists()) {
        Some(found) => Ok(found),
        None => {
            tracing::error!(
                model,
                searched = ?["models directory", "current directory", "./models"],
                "Model file not found; download it from https://huggingface.co/ggerganov/whisper.cpp/tree/main"
            );
            Err(TranscribeError::ModelNotFound)
        }
    }
}

fn calculate_audio_ctx(duration_secs: f32) -> Option<i32> {
    const MIN_AUDIO_CTX: i32 = 384;

    if duration_secs <= 22.5 {
        let raw_ctx = (duration_secs * 50.0) as i32 + 128;
        let bounded_ctx = raw_ctx.max(MIN_AUDIO_CTX);
        Some((bounded_ctx + 7) / 8 * 8)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_audio_ctx_short_clips() {
        assert_eq!(calculate_audio_ctx(1.0), Some(384));
        assert_eq!(calculate_audio_ctx(5.0), Some(384));
        assert_eq!(calculate_audio_ctx(10.0), Some(632));
        assert_eq!(calculate_audio_ctx(22.5), Some(1256));
    }

    #[test]
    fn test_calculate_audio_ctx_long_clips() {
        assert_eq!(calculate_audio_ctx(22.6), None);
        assert_eq!(calculate_audio_ctx(30.0), None);
        assert_eq!(calculate_audio_ctx(60.0), None);
    }

    #[test]
    fn test_audio_ctx_not_applied_when_disabled() {
        const WHISPER_DEFAULT_AUDIO_CTX: i32 = 1500;

        let optimized_ctx = calculate_audio_ctx(1.0);
        assert_eq!(optimized_ctx, Some(384));
        assert!(optimized_ctx.unwrap() < WHISPER_DEFAULT_AUDIO_CTX);

        let ratio = WHISPER_DEFAULT_AUDIO_CTX as f32 / optimized_ctx.unwrap() as f32;
        assert!(
            ratio > 3.0,
            "Optimization should reduce context by >3x for 1s clips"
        );
    }

    #[test]
    fn test_audio_ctx_alignment() {
        [1.0, 3.0, 5.0, 7.0, 10.0, 15.0, 20.0, 22.5]
            .into_iter()
            .filter_map(calculate_audio_ctx)
            .for_each(|ctx| assert_eq!(ctx % 8, 0, "audio_ctx {ctx} should be aligned to 8"));
    }

    #[test]
    fn test_degenerate_transcript_detection() {
        assert!(is_degenerate_transcript(""));
        assert!(is_degenerate_transcript("   "));
        assert!(is_degenerate_transcript("-"));
        assert!(is_degenerate_transcript(" - \n"));
        assert!(is_degenerate_transcript("..."));
        assert!(is_degenerate_transcript("—"));

        assert!(!is_degenerate_transcript("- Implement phase 1"));
        assert!(!is_degenerate_transcript("hello"));
        assert!(!is_degenerate_transcript("123"));
    }
}
