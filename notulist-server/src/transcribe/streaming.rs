use crate::error::TranscribeError;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

#[derive(Debug)]
pub enum StreamingEvent {
    Partial { text: String },
    Final { text: String },
    Replace { backspace: usize, text: String },
    Decoding(bool),
    Ended,
    Error(TranscribeError),
}

pub struct StreamHandle {
    pub events: mpsc::Receiver<StreamingEvent>,
    pub cancel: oneshot::Sender<()>,
    pub task: JoinHandle<()>,
}
