use clap::Subcommand;
use futures_util::StreamExt;
use notulist_client::{
    connect_dictation_proxy, DictationProxy, SessionEndedStream, TranscriptAccumulator,
    TranscriptEditedStream,
};

use crate::error::DaemonError;

#[derive(Debug, Clone, Copy, Subcommand)]
pub enum RecordAction {
    #[command(about = "Start listening")]
    Start,
    #[command(about = "Stop listening and let the transcript finish")]
    Stop,
    #[command(about = "Start when idle, stop when listening")]
    Toggle,
    #[command(about = "Abort the session and discard its transcript")]
    Cancel,
}

pub async fn record(action: RecordAction, wait_for_transcript: bool) -> Result<(), DaemonError> {
    let proxy = connect_dictation_proxy().await?;
    let transcript = if wait_for_transcript {
        Some(SessionTranscript::subscribe(&proxy).await?)
    } else {
        None
    };
    match action {
        RecordAction::Start => proxy.start().await?,
        RecordAction::Stop => proxy.stop().await?,
        RecordAction::Toggle => proxy.toggle().await?,
        RecordAction::Cancel => proxy.cancel().await?,
    }
    if let Some(transcript) = transcript {
        println!("{}", transcript.collect().await?);
    }
    Ok(())
}

struct SessionTranscript<'a> {
    edits: TranscriptEditedStream<'a>,
    endings: SessionEndedStream<'a>,
}

impl<'a> SessionTranscript<'a> {
    async fn subscribe(proxy: &'a DictationProxy<'static>) -> zbus::Result<Self> {
        Ok(Self {
            edits: proxy.receive_transcript_edited().await?,
            endings: proxy.receive_session_ended().await?,
        })
    }

    async fn collect(mut self) -> zbus::Result<String> {
        let mut transcript = TranscriptAccumulator::default();
        loop {
            tokio::select! {
                Some(edit) = self.edits.next() => transcript.apply(&edit.args()?.edit),
                _ = self.endings.next() => return Ok(transcript.into_text()),
            }
        }
    }
}

pub async fn status(follow: bool) -> Result<(), DaemonError> {
    let proxy = connect_dictation_proxy().await?;
    let mut phase_changes = proxy.receive_phase_changed().await?;
    println!("{}", proxy.current_phase().await?.label());
    if !follow {
        return Ok(());
    }
    while let Some(change) = phase_changes.next().await {
        println!("{}", change.args()?.phase.label());
    }
    Ok(())
}
