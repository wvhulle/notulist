use notulist_client::{
    DictationFailure, DictationPhase, LevelFrame, SessionOutcome, TranscriptEdit,
};
use zbus::SignalContext;

use crate::dbus_dictation_service::DictationService;

#[derive(Clone)]
pub struct DictationSignalEmitter {
    context: SignalContext<'static>,
}

impl DictationSignalEmitter {
    pub fn new(context: SignalContext<'static>) -> Self {
        Self { context }
    }

    pub async fn phase_changed(&self, phase: DictationPhase) {
        log_if_failed(
            "PhaseChanged",
            DictationService::phase_changed(&self.context, phase).await,
        );
    }

    pub async fn levels(&self, frames: Vec<LevelFrame>) {
        log_if_failed(
            "Levels",
            DictationService::levels(&self.context, frames).await,
        );
    }

    pub async fn transcript_edited(&self, edit: TranscriptEdit) {
        log_if_failed(
            "TranscriptEdited",
            DictationService::transcript_edited(&self.context, edit).await,
        );
    }

    pub async fn decoding_changed(&self, active: bool) {
        log_if_failed(
            "DecodingChanged",
            DictationService::decoding_changed(&self.context, active).await,
        );
    }

    pub async fn session_failed(&self, failure: DictationFailure) {
        log_if_failed(
            "SessionFailed",
            DictationService::session_failed(&self.context, failure).await,
        );
    }

    pub async fn session_ended(&self, outcome: SessionOutcome) {
        log_if_failed(
            "SessionEnded",
            DictationService::session_ended(&self.context, outcome).await,
        );
    }
}

fn log_if_failed(signal: &'static str, emitted: zbus::Result<()>) {
    if let Err(error) = emitted {
        tracing::warn!(signal, %error, "Could not emit a dictation signal");
    }
}
