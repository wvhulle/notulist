use tokio::sync::watch;

use super::{text_field_focus_changes, InputMethodFieldWriter, TextFieldFocus};
use crate::config::OutputConfig;
use crate::output::streaming::StreamingSession;

pub struct DictationFieldIntegration {
    focus_changes: Option<watch::Receiver<TextFieldFocus>>,
}

impl DictationFieldIntegration {
    pub fn new(config: &OutputConfig) -> Self {
        Self {
            focus_changes: text_field_focus_changes(config),
        }
    }

    pub async fn next_focused_field(&mut self) -> Option<u32> {
        let Some(changes) = self.focus_changes.as_mut() else {
            return std::future::pending().await;
        };
        if changes.changed().await.is_err() {
            self.focus_changes = None;
            return None;
        }
        changes.borrow().focused_field()
    }

    pub fn session_field_lost(
        &self,
        focused_field: Option<u32>,
        streaming: bool,
        session: Option<&StreamingSession>,
    ) -> bool {
        let dictated_field = session.and_then(StreamingSession::input_method_field);
        let lost = streaming && dictated_field.is_some() && dictated_field != focused_field;
        if lost {
            tracing::info!("Text field lost focus; stopping dictation");
        }
        lost
    }
}

pub fn field_writer_for_session(
    config: &OutputConfig,
    file_output: bool,
) -> Option<InputMethodFieldWriter> {
    if file_output {
        return None;
    }
    let writer = InputMethodFieldWriter::for_focused_field(config);
    if writer.is_some() {
        tracing::info!("Streaming into the focused text field through the input method");
    } else if crate::output::input_method_driver_requested(config) {
        tracing::info!(
            "No focused text field accepts input-method text; typing through the virtual keyboard"
        );
    }
    writer
}
