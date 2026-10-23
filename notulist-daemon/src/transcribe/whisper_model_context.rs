use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Once;

use notulist_whisper_sys as whisper_ffi;

use crate::error::TranscribeError;

static GGML_BACKENDS_LOADED: Once = Once::new();

pub struct ContextOptions {
    pub gpu_device_index: Option<i32>,
    pub flash_attention: bool,
}

pub struct DecodingRequest<'request> {
    pub language: Option<&'request str>,
    pub translate_to_english: bool,
    pub inference_threads: i32,
    pub initial_prompt: Option<&'request str>,
    pub single_segment: bool,
    pub audio_context_size: Option<i32>,
    pub use_beam_search: bool,
}

pub struct LanguageDetection {
    pub detected_language_id: i32,
    pub probabilities: Vec<f32>,
}

pub struct WhisperModelContext {
    raw_context: NonNull<whisper_ffi::whisper_context>,
}

unsafe impl Send for WhisperModelContext {}
unsafe impl Sync for WhisperModelContext {}

impl WhisperModelContext {
    pub fn load(model_path: &Path, options: &ContextOptions) -> Result<Self, TranscribeError> {
        load_ggml_backends_once();
        let Some(model_path_text) = model_path.to_str() else {
            return Err(TranscribeError::ModelPathNotUtf8);
        };
        let model_path_c = CString::new(model_path_text)?;
        let mut context_params = unsafe { whisper_ffi::whisper_context_default_params() };
        context_params.use_gpu = true;
        context_params.flash_attn = options.flash_attention;
        if let Some(gpu_device_index) = options.gpu_device_index {
            context_params.gpu_device = gpu_device_index;
        }
        let raw_context = unsafe {
            whisper_ffi::whisper_init_from_file_with_params(model_path_c.as_ptr(), context_params)
        };
        let Some(raw_context) = NonNull::new(raw_context) else {
            return Err(TranscribeError::ContextInitFailed);
        };
        Ok(Self { raw_context })
    }

    pub fn new_decoding_state(&self) -> Result<DecodingState<'_>, TranscribeError> {
        let raw_state = unsafe { whisper_ffi::whisper_init_state(self.raw_context.as_ptr()) };
        let Some(raw_state) = NonNull::new(raw_state) else {
            return Err(TranscribeError::StateInitFailed);
        };
        Ok(DecodingState {
            raw_state,
            model_context: self,
        })
    }
}

impl Drop for WhisperModelContext {
    fn drop(&mut self) {
        unsafe { whisper_ffi::whisper_free(self.raw_context.as_ptr()) };
    }
}

pub struct DecodingState<'context> {
    raw_state: NonNull<whisper_ffi::whisper_state>,
    model_context: &'context WhisperModelContext,
}

impl DecodingState<'_> {
    pub fn detect_language(
        &mut self,
        samples: &[f32],
        inference_threads: i32,
    ) -> Result<LanguageDetection, TranscribeError> {
        let sample_count = i32::try_from(samples.len())?;
        let mel_status = unsafe {
            whisper_ffi::whisper_pcm_to_mel_with_state(
                self.model_context.raw_context.as_ptr(),
                self.raw_state.as_ptr(),
                samples.as_ptr(),
                sample_count,
                inference_threads,
            )
        };
        if mel_status != 0 {
            return Err(TranscribeError::MelConversionFailed);
        }
        let language_count = unsafe { whisper_ffi::whisper_lang_max_id() } + 1;
        let mut probabilities = vec![0.0_f32; usize::try_from(language_count)?];
        let detected_language_id = unsafe {
            whisper_ffi::whisper_lang_auto_detect_with_state(
                self.model_context.raw_context.as_ptr(),
                self.raw_state.as_ptr(),
                0,
                inference_threads,
                probabilities.as_mut_ptr(),
            )
        };
        if detected_language_id < 0 {
            return Err(TranscribeError::LanguageDetectionFailed);
        }
        Ok(LanguageDetection {
            detected_language_id,
            probabilities,
        })
    }

    pub fn decode(
        &mut self,
        samples: &[f32],
        request: &DecodingRequest,
    ) -> Result<String, TranscribeError> {
        let language_c = request.language.map(CString::new).transpose()?;
        let initial_prompt_c = request.initial_prompt.map(CString::new).transpose()?;
        let sampling_strategy = if request.use_beam_search {
            whisper_ffi::whisper_sampling_strategy::WHISPER_SAMPLING_BEAM_SEARCH
        } else {
            whisper_ffi::whisper_sampling_strategy::WHISPER_SAMPLING_GREEDY
        };
        let mut full_params =
            unsafe { whisper_ffi::whisper_full_default_params(sampling_strategy) };
        full_params.n_threads = request.inference_threads;
        full_params.translate = request.translate_to_english;
        full_params.no_context = true;
        full_params.single_segment = request.single_segment;
        full_params.print_special = false;
        full_params.print_progress = false;
        full_params.print_realtime = false;
        full_params.print_timestamps = false;
        full_params.suppress_blank = true;
        full_params.suppress_nst = true;
        full_params.greedy.best_of = 1;
        full_params.beam_search.beam_size = 5;
        full_params.beam_search.patience = -1.0;
        full_params.audio_ctx = request.audio_context_size.unwrap_or(0);
        full_params.language = language_c
            .as_ref()
            .map_or(std::ptr::null(), |language| language.as_ptr());
        full_params.detect_language = false;
        full_params.initial_prompt = initial_prompt_c
            .as_ref()
            .map_or(std::ptr::null(), |prompt| prompt.as_ptr());

        let sample_count = i32::try_from(samples.len())?;
        let decode_status = unsafe {
            whisper_ffi::whisper_full_with_state(
                self.model_context.raw_context.as_ptr(),
                self.raw_state.as_ptr(),
                full_params,
                samples.as_ptr(),
                sample_count,
            )
        };
        if decode_status != 0 {
            return Err(TranscribeError::InferenceFailed);
        }
        let segment_count =
            unsafe { whisper_ffi::whisper_full_n_segments_from_state(self.raw_state.as_ptr()) };
        let text = (0..segment_count)
            .map(|segment_index| self.segment_text(segment_index))
            .collect::<Result<String, TranscribeError>>()?;
        Ok(text.trim().to_string())
    }

    fn segment_text(&self, segment_index: i32) -> Result<String, TranscribeError> {
        let raw_text = unsafe {
            whisper_ffi::whisper_full_get_segment_text_from_state(
                self.raw_state.as_ptr(),
                segment_index,
            )
        };
        if raw_text.is_null() {
            return Ok(String::new());
        }
        Ok(unsafe { CStr::from_ptr(raw_text) }.to_str()?.to_owned())
    }
}

impl Drop for DecodingState<'_> {
    fn drop(&mut self) {
        unsafe { whisper_ffi::whisper_free_state(self.raw_state.as_ptr()) };
    }
}

pub fn language_id_for_code(language_code: &str) -> Option<usize> {
    let language_code_c = CString::new(language_code).ok()?;
    let language_id = unsafe { whisper_ffi::whisper_lang_id(language_code_c.as_ptr()) };
    usize::try_from(language_id).ok()
}

pub fn language_code_for_id(language_id: i32) -> Option<&'static str> {
    let raw_code = unsafe { whisper_ffi::whisper_lang_str(language_id) };
    if raw_code.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(raw_code) }.to_str().ok()
}

fn load_ggml_backends_once() {
    GGML_BACKENDS_LOADED.call_once(|| unsafe {
        whisper_ffi::whisper_log_set(Some(forward_whisper_log), std::ptr::null_mut());
        whisper_ffi::ggml_backend_load_all();
    });
}

unsafe extern "C" fn forward_whisper_log(
    _level: whisper_ffi::ggml_log_level,
    text: *const c_char,
    _user_data: *mut c_void,
) {
    if text.is_null() {
        return;
    }
    let message = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    tracing::trace!(target: "notulist::whisper_cpp", "{}", message.trim_end());
}
