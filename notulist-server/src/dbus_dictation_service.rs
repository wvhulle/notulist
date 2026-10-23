use notulist_client::{
    DictationFailure, DictationPhase, LevelFrame, SessionOutcome, TranscriptEdit,
    DICTATION_BUS_NAME, DICTATION_OBJECT_PATH,
};
use tokio::sync::{mpsc, watch};
use zbus::{fdo, SignalContext};

use crate::error::DaemonError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictationCommand {
    Start,
    Stop,
    Toggle,
    Cancel,
}

pub struct DictationService {
    commands: mpsc::Sender<DictationCommand>,
    phase: watch::Receiver<DictationPhase>,
}

impl DictationService {
    async fn forward(&self, command: DictationCommand) -> fdo::Result<()> {
        if self.commands.send(command).await.is_err() {
            return Err(fdo::Error::Failed(
                "the dictation controller has stopped".into(),
            ));
        }
        Ok(())
    }
}

#[zbus::interface(name = "io.github.wvhulle.Notulist.Dictation1")]
impl DictationService {
    async fn start(&self) -> fdo::Result<()> {
        self.forward(DictationCommand::Start).await
    }

    async fn stop(&self) -> fdo::Result<()> {
        self.forward(DictationCommand::Stop).await
    }

    async fn toggle(&self) -> fdo::Result<()> {
        self.forward(DictationCommand::Toggle).await
    }

    async fn cancel(&self) -> fdo::Result<()> {
        self.forward(DictationCommand::Cancel).await
    }

    async fn current_phase(&self) -> DictationPhase {
        *self.phase.borrow()
    }

    #[zbus(signal)]
    pub async fn phase_changed(
        signal_context: &SignalContext<'_>,
        phase: DictationPhase,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn levels(
        signal_context: &SignalContext<'_>,
        frames: Vec<LevelFrame>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn transcript_edited(
        signal_context: &SignalContext<'_>,
        edit: TranscriptEdit,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn session_ended(
        signal_context: &SignalContext<'_>,
        outcome: SessionOutcome,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn decoding_changed(
        signal_context: &SignalContext<'_>,
        active: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn session_failed(
        signal_context: &SignalContext<'_>,
        failure: DictationFailure,
    ) -> zbus::Result<()>;
}

pub struct DictationBusEndpoint {
    pub connection: zbus::Connection,
    pub signal_context: SignalContext<'static>,
}

pub async fn serve_dictation_on_session_bus(
    commands: mpsc::Sender<DictationCommand>,
    phase: watch::Receiver<DictationPhase>,
) -> Result<DictationBusEndpoint, DaemonError> {
    let service = DictationService { commands, phase };
    let connection = zbus::connection::Builder::session()?
        .serve_at(DICTATION_OBJECT_PATH, service)?
        .build()
        .await?;
    let name_request = connection
        .request_name_with_flags(DICTATION_BUS_NAME, fdo::RequestNameFlags::DoNotQueue.into())
        .await;
    match name_request {
        Ok(fdo::RequestNameReply::PrimaryOwner | fdo::RequestNameReply::AlreadyOwner) => {}
        Ok(_) | Err(zbus::Error::NameTaken) => return Err(DaemonError::NameTaken),
        Err(error) => return Err(DaemonError::Bus(error)),
    }
    let signal_context = SignalContext::new(&connection, DICTATION_OBJECT_PATH)?.into_owned();
    Ok(DictationBusEndpoint {
        connection,
        signal_context,
    })
}
