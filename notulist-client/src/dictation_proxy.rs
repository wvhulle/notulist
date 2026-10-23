use crate::dictation_types::{
    DictationFailure, DictationPhase, LevelFrame, SessionOutcome, TranscriptEdit,
};

#[zbus::proxy(
    interface = "io.github.wvhulle.Notulist.Dictation1",
    default_service = "io.github.wvhulle.Notulist",
    default_path = "/io/github/wvhulle/Notulist"
)]
pub trait Dictation {
    fn start(&self) -> zbus::Result<()>;

    fn stop(&self) -> zbus::Result<()>;

    fn toggle(&self) -> zbus::Result<()>;

    fn cancel(&self) -> zbus::Result<()>;

    fn current_phase(&self) -> zbus::Result<DictationPhase>;

    #[zbus(signal)]
    fn phase_changed(&self, phase: DictationPhase) -> zbus::Result<()>;

    #[zbus(signal)]
    fn levels(&self, frames: Vec<LevelFrame>) -> zbus::Result<()>;

    #[zbus(signal)]
    fn transcript_edited(&self, edit: TranscriptEdit) -> zbus::Result<()>;

    #[zbus(signal)]
    fn session_ended(&self, outcome: SessionOutcome) -> zbus::Result<()>;

    #[zbus(signal)]
    fn decoding_changed(&self, active: bool) -> zbus::Result<()>;

    #[zbus(signal)]
    fn session_failed(&self, failure: DictationFailure) -> zbus::Result<()>;
}

pub async fn connect_dictation_proxy() -> zbus::Result<DictationProxy<'static>> {
    let connection = zbus::Connection::session().await?;
    DictationProxy::new(&connection).await
}
