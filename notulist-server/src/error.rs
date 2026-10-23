use std::io;
use std::sync::mpsc::RecvError;
use std::time::TryFromFloatSecsError;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read the config file")]
    Read(#[from] io::Error),
    #[error("could not parse the config file; check the TOML syntax and the key names")]
    Parse(#[from] toml::de::Error),
    #[error("the config file given on the command line does not exist")]
    ExplicitFileMissing,
}

#[derive(Debug, thiserror::Error)]
pub enum MicrophoneError {
    #[error("no default microphone is available; check your audio setup or set [daemon.microphone] device_name")]
    NoDefaultDevice,
    #[error(
        "the configured microphone was not found; list devices with: pactl list sources short"
    )]
    DeviceNotFound,
    #[error("could not enumerate the audio input devices")]
    DeviceList(#[from] cpal::DevicesError),
    #[error("the microphone did not report a usable input format")]
    FormatUnavailable(#[from] cpal::DefaultStreamConfigError),
    #[error("the microphone uses an unsupported sample format: {0:?}")]
    UnsupportedSampleFormat(cpal::SampleFormat),
    #[error("could not open the microphone stream")]
    Stream(#[from] cpal::BuildStreamError),
    #[error("could not start the microphone stream")]
    Play(#[from] cpal::PlayStreamError),
    #[error("the microphone was disconnected during the session")]
    Disconnected,
    #[error("the capture thread stopped before the microphone was ready")]
    CaptureThreadStopped(#[from] RecvError),
}

#[derive(Debug, thiserror::Error)]
pub enum ResamplerError {
    #[error("could not build the sample-rate converter")]
    Construct(#[from] rubato::ResamplerConstructionError),
}

#[derive(Debug, thiserror::Error)]
pub enum TranscribeError {
    #[error(
        "model file not found; set [daemon.whisper] model to a ggml .bin path or a model name"
    )]
    ModelNotFound,
    #[error("unknown model name; use a ggml .bin path or one of: tiny, base, small, medium, large-v3, large-v3-turbo")]
    UnknownModelName,
    #[error("the model path is not valid UTF-8")]
    ModelPathNotUtf8,
    #[error("the built-in whisper model table is malformed")]
    ModelTable(#[from] toml::de::Error),
    #[error("could not download the whisper model")]
    ModelDownload(#[from] Box<ureq::Error>),
    #[error("could not store the whisper model")]
    ModelStorage(#[from] std::io::Error),
    #[error("the downloaded whisper model has the wrong checksum")]
    ModelChecksumMismatch,
    #[error("a path, language code or prompt contains a NUL byte")]
    ContainsNulByte(#[from] std::ffi::NulError),
    #[error("whisper.cpp could not load the model; check the file and the GPU backend")]
    ContextInitFailed,
    #[error("whisper.cpp could not allocate a decoding state")]
    StateInitFailed,
    #[error("whisper.cpp could not compute the mel spectrogram")]
    MelConversionFailed,
    #[error("whisper.cpp could not detect the spoken language")]
    LanguageDetectionFailed,
    #[error("whisper.cpp inference failed")]
    InferenceFailed,
    #[error("the whisper model is unusable after a panic in an earlier pass; restart the daemon")]
    ModelBusy,
    #[error("whisper.cpp returned text that is not valid UTF-8")]
    SegmentTextNotUtf8(#[from] std::str::Utf8Error),
    #[error("the audio buffer is too long for whisper.cpp")]
    AudioTooLong(#[from] std::num::TryFromIntError),
}

#[derive(Debug, thiserror::Error)]
pub enum ShortcutError {
    #[error("[daemon.shortcut] contains an unknown evdev key name")]
    UnknownKeyName,
    #[error("could not list the input devices in /dev/input")]
    InputDirectoryUnreadable(#[from] io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error(transparent)]
    Microphone(#[from] MicrophoneError),
    #[error("could not read the WAV file")]
    WavFile(#[from] hound::Error),
    #[error("playback speed must be a positive, finite number")]
    PlaybackSpeed(#[from] TryFromFloatSecsError),
}

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Transcribe(#[from] TranscribeError),
    #[error(transparent)]
    Shortcut(#[from] ShortcutError),
    #[error("could not use the session bus")]
    Bus(#[from] zbus::Error),
    #[error("another notulist-server already owns io.github.wvhulle.Notulist on the session bus; stop the running `notulist-server` first")]
    NameTaken,
}
