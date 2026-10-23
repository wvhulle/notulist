use std::sync::Arc;
use std::time::Duration;

use notulist_client::DictationPhase;
use tokio::sync::{mpsc, oneshot, watch};

use crate::audio::AudioSource;
use crate::config::DaemonConfig;
use crate::dbus_dictation_service::{serve_dictation_on_session_bus, DictationCommand};
use crate::dictation_session_controller::{DictationSessionController, LoadedEngine};
use crate::dictation_signal_emitter::DictationSignalEmitter;
use crate::error::DaemonError;
use crate::spoken_text_processor::TextProcessor;
use crate::transcribe::sliding_window::{SlidingWindowConfig, SlidingWindowStreamingTranscriber};
use crate::transcribe::whisper::WhisperTranscriber;

const COMMAND_QUEUE_DEPTH: usize = 16;

pub struct DictationDaemon {
    endpoint: crate::dbus_dictation_service::DictationBusEndpoint,
    controller: DictationSessionController,
    commands: mpsc::Receiver<DictationCommand>,
    engine: oneshot::Receiver<LoadedEngine>,
}

impl DictationDaemon {
    pub async fn bind(config: DaemonConfig, source: AudioSource) -> Result<Self, DaemonError> {
        let (commands_tx, commands_rx) = mpsc::channel(COMMAND_QUEUE_DEPTH);
        let (phase_tx, phase_rx) = watch::channel(DictationPhase::LoadingModel);
        let endpoint = serve_dictation_on_session_bus(commands_tx, phase_rx).await?;
        tracing::info!(
            "Listening on the session bus as {}",
            notulist_client::DICTATION_BUS_NAME
        );

        let engine = load_engine_in_background(&config, source.sample_rate_hz());
        let max_session = Duration::from_secs(u64::from(config.microphone.max_session_secs));
        let controller = DictationSessionController::new(
            source,
            max_session,
            Arc::new(TextProcessor::new(&config.text_cleanup)),
            DictationSignalEmitter::new(endpoint.signal_context.clone()),
            phase_tx,
        );
        Ok(Self {
            endpoint,
            controller,
            commands: commands_rx,
            engine,
        })
    }

    pub async fn run(self) {
        let Self {
            endpoint,
            controller,
            commands,
            engine,
        } = self;
        controller.run(commands, engine).await;
        drop(endpoint);
    }
}

fn load_engine_in_background(
    config: &DaemonConfig,
    sample_rate_hz: u32,
) -> oneshot::Receiver<LoadedEngine> {
    let (engine_tx, engine_rx) = oneshot::channel();
    let whisper_config = config.whisper.clone();
    let sliding_config =
        SlidingWindowConfig::from_streaming_config(&config.streaming, sample_rate_hz);
    std::thread::spawn(move || {
        let loaded = WhisperTranscriber::new(&whisper_config).map(|whisper| {
            SlidingWindowStreamingTranscriber::new(Arc::new(whisper), sliding_config)
        });
        let _ = engine_tx.send(loaded);
    });
    engine_rx
}
