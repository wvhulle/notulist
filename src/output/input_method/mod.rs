pub mod daemon_integration;
pub mod preedit_commit_tracker;
#[cfg(feature = "input-method")]
pub mod wayland_input_method_connection;

use tokio::sync::watch;

use crate::config::OutputConfig;
use crate::error::OutputError;
use crate::output::TextOutput;
use preedit_commit_tracker::{InputMethodBatch, PreeditCommitTracker};

#[cfg(feature = "input-method")]
type FieldConnection = &'static wayland_input_method_connection::InputMethodConnection;
#[cfg(not(feature = "input-method"))]
type FieldConnection = std::convert::Infallible;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFieldFocus {
    Connecting,
    Unfocused,
    Focused { field: u32 },
    Unavailable,
}

impl TextFieldFocus {
    pub fn focused_field(self) -> Option<u32> {
        match self {
            TextFieldFocus::Focused { field } => Some(field),
            TextFieldFocus::Connecting
            | TextFieldFocus::Unfocused
            | TextFieldFocus::Unavailable => None,
        }
    }
}

pub struct InputMethodFieldWriter {
    tracker: PreeditCommitTracker,
    connection: FieldConnection,
    field: u32,
}

impl InputMethodFieldWriter {
    pub fn for_focused_field(config: &OutputConfig) -> Option<Self> {
        if !super::input_method_driver_requested(config) {
            return None;
        }
        let (connection, field) = focused_field_connection()?;
        Some(Self {
            tracker: PreeditCommitTracker::new(
                crate::transcribe::sliding_window::REVISION_LAG_WORDS + 1,
            ),
            connection,
            field,
        })
    }

    pub fn field(&self) -> u32 {
        self.field
    }

    pub fn append(&mut self, text: &str) -> Result<(), OutputError> {
        let batch = self.tracker.append(text);
        self.send(batch)
    }

    pub fn replace_trailing_chars(
        &mut self,
        removed_chars: usize,
        text: &str,
    ) -> Result<(), OutputError> {
        let batch = self.tracker.replace_trailing_chars(removed_chars, text);
        self.send(batch)
    }

    pub fn erase_everything(&mut self) -> Result<(), OutputError> {
        let batch = self.tracker.erase_everything();
        self.send(batch)
    }

    #[cfg(feature = "input-method")]
    fn send(&self, batch: InputMethodBatch) -> Result<(), OutputError> {
        if self.connection.send(self.field, batch) {
            Ok(())
        } else {
            Err(OutputError::TextFieldNotFocused)
        }
    }

    #[cfg(not(feature = "input-method"))]
    fn send(&self, _batch: InputMethodBatch) -> Result<(), OutputError> {
        match self.connection {}
    }
}

impl Drop for InputMethodFieldWriter {
    fn drop(&mut self) {
        let batch = self.tracker.commit_everything();
        if !batch.commit_text.is_empty() && self.send(batch).is_err() {
            tracing::debug!(
                "Text field lost focus before the provisional words could be committed"
            );
        }
    }
}

#[cfg(feature = "input-method")]
fn focused_field_connection() -> Option<(FieldConnection, u32)> {
    let connection = wayland_input_method_connection::InputMethodConnection::shared();
    connection.focused_field().map(|field| (connection, field))
}

#[cfg(not(feature = "input-method"))]
fn focused_field_connection() -> Option<(FieldConnection, u32)> {
    None
}

#[cfg(feature = "input-method")]
pub fn text_field_focus_changes(config: &OutputConfig) -> Option<watch::Receiver<TextFieldFocus>> {
    super::input_method_driver_requested(config)
        .then(|| wayland_input_method_connection::InputMethodConnection::shared().focus_changes())
}

#[cfg(not(feature = "input-method"))]
pub fn text_field_focus_changes(_config: &OutputConfig) -> Option<watch::Receiver<TextFieldFocus>> {
    None
}

pub struct InputMethodOutput;

#[async_trait::async_trait]
impl TextOutput for InputMethodOutput {
    async fn output(&self, text: &str) -> Result<(), OutputError> {
        send_one_shot(InputMethodBatch {
            commit_text: text.to_owned(),
            ..InputMethodBatch::default()
        })
    }

    async fn is_available(&self) -> bool {
        focused_field_connection().is_some()
    }

    fn name(&self) -> &'static str {
        "input-method"
    }
}

#[cfg(feature = "input-method")]
fn send_one_shot(batch: InputMethodBatch) -> Result<(), OutputError> {
    match focused_field_connection() {
        Some((connection, field)) if connection.send(field, batch) => Ok(()),
        _ => Err(OutputError::TextFieldNotFocused),
    }
}

#[cfg(not(feature = "input-method"))]
fn send_one_shot(_batch: InputMethodBatch) -> Result<(), OutputError> {
    Err(OutputError::TextFieldNotFocused)
}
