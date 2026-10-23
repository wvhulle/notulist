use std::sync::Arc;
use std::time::Duration;

use notulist_client::{DictationFailure, DictationPhase, SessionOutcome};
use tokio::sync::oneshot::error::RecvError;
use tokio::sync::{mpsc, oneshot, watch};

use crate::audio::AudioSource;
use crate::dbus_dictation_service::DictationCommand;
use crate::dictation_session_pipeline::{
    start_dictation_session, SessionControls, SessionMilestone,
};
use crate::dictation_signal_emitter::DictationSignalEmitter;
use crate::error::TranscribeError;
use crate::spoken_text_processor::TextProcessor;
use crate::transcribe::sliding_window::SlidingWindowStreamingTranscriber;

const SESSION_MILESTONE_QUEUE_DEPTH: usize = 16;

pub type LoadedEngine = Result<SlidingWindowStreamingTranscriber, TranscribeError>;

pub struct DictationSessionController {
    source: AudioSource,
    max_session: Duration,
    streaming: Option<SlidingWindowStreamingTranscriber>,
    text_processor: Arc<TextProcessor>,
    signals: DictationSignalEmitter,
    phase: watch::Sender<DictationPhase>,
    active_session: Option<SessionControls>,
}

impl DictationSessionController {
    pub fn new(
        source: AudioSource,
        max_session: Duration,
        text_processor: Arc<TextProcessor>,
        signals: DictationSignalEmitter,
        phase: watch::Sender<DictationPhase>,
    ) -> Self {
        Self {
            source,
            max_session,
            streaming: None,
            text_processor,
            signals,
            phase,
            active_session: None,
        }
    }

    pub async fn run(
        mut self,
        mut commands: mpsc::Receiver<DictationCommand>,
        mut engine: oneshot::Receiver<LoadedEngine>,
    ) {
        let (milestones_tx, mut milestones_rx) = mpsc::channel(SESSION_MILESTONE_QUEUE_DEPTH);
        let mut engine_loading = true;
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.handle_command(command, &milestones_tx).await,
                    None => break,
                },
                loaded = &mut engine, if engine_loading => {
                    engine_loading = false;
                    self.engine_loaded(loaded).await;
                }
                Some(milestone) = milestones_rx.recv() => self.handle_milestone(milestone).await,
            }
        }
        if let Some(session) = self.active_session.as_mut() {
            session.request_cancel();
        }
    }

    async fn engine_loaded(&mut self, loaded: Result<LoadedEngine, RecvError>) {
        match loaded {
            Ok(Ok(engine)) => {
                self.streaming = Some(engine);
                self.enter_phase(DictationPhase::Idle).await;
            }
            Ok(Err(error)) => {
                tracing::error!(%error, "Could not load the whisper model");
                self.fail_session(DictationFailure::ModelNotLoaded, SessionOutcome::Cancelled)
                    .await;
            }
            Err(_) => {
                tracing::error!("The whisper model loader stopped without a result");
                self.fail_session(DictationFailure::ModelNotLoaded, SessionOutcome::Cancelled)
                    .await;
            }
        }
    }

    async fn handle_command(
        &mut self,
        command: DictationCommand,
        milestones: &mpsc::Sender<SessionMilestone>,
    ) {
        let phase = *self.phase.borrow();
        match (command, phase) {
            (DictationCommand::Start | DictationCommand::Toggle, phase)
                if phase.accepts_new_session() =>
            {
                self.start_session(milestones).await
            }
            (DictationCommand::Stop | DictationCommand::Toggle, DictationPhase::Listening) => {
                self.request_stop()
            }
            (DictationCommand::Cancel, DictationPhase::Listening | DictationPhase::Finishing) => {
                self.request_cancel()
            }
            (command, DictationPhase::LoadingModel) => {
                tracing::info!(
                    ?command,
                    "Ignoring command: the whisper model is still loading"
                );
            }
            (command, phase) => {
                tracing::debug!(?command, ?phase, "Ignoring command in this phase");
            }
        }
    }

    async fn start_session(&mut self, milestones: &mpsc::Sender<SessionMilestone>) {
        let Some(engine) = self.streaming.as_ref() else {
            tracing::error!("Cannot start dictation: the whisper model is not loaded");
            self.fail_session(DictationFailure::ModelNotLoaded, SessionOutcome::Cancelled)
                .await;
            return;
        };
        let started = start_dictation_session(
            &self.source,
            self.max_session,
            engine,
            Arc::clone(&self.text_processor),
            self.signals.clone(),
            milestones.clone(),
        );
        match started {
            Ok(session) => {
                self.active_session = Some(session);
                self.enter_phase(DictationPhase::Listening).await;
            }
            Err(error) => {
                tracing::error!(%error, "Could not start dictation");
                self.fail_session(
                    DictationFailure::MicrophoneUnavailable,
                    SessionOutcome::Cancelled,
                )
                .await;
            }
        }
    }

    fn request_stop(&mut self) {
        if let Some(session) = self.active_session.as_mut() {
            session.request_stop();
        }
    }

    fn request_cancel(&mut self) {
        if let Some(session) = self.active_session.as_mut() {
            session.request_cancel();
        }
    }

    async fn handle_milestone(&mut self, milestone: SessionMilestone) {
        match milestone {
            SessionMilestone::AudioEnded => self.enter_phase(DictationPhase::Finishing).await,
            SessionMilestone::Finished(outcome) => {
                self.active_session = None;
                self.signals.session_ended(outcome).await;
                self.enter_phase(DictationPhase::Idle).await;
            }
            SessionMilestone::Failed(failure) => {
                self.fail_session(failure, SessionOutcome::Committed).await;
            }
        }
    }

    async fn fail_session(&mut self, failure: DictationFailure, outcome: SessionOutcome) {
        self.active_session = None;
        self.signals.session_failed(failure).await;
        self.signals.session_ended(outcome).await;
        self.enter_phase(DictationPhase::Failed).await;
    }

    async fn enter_phase(&mut self, phase: DictationPhase) {
        if *self.phase.borrow() == phase {
            return;
        }
        self.phase.send_replace(phase);
        tracing::info!("Dictation phase: {}", phase.label());
        self.signals.phase_changed(phase).await;
    }
}
