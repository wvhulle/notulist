use std::time::{Duration, Instant};

use futures_util::StreamExt;
use notulist_client::{
    DictationFailure, DictationPhase, DictationProxy, LevelFrame, SessionOutcome, TranscriptEdit,
};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::dictation_typing_session::{DictationTypingSession, TypingAction};
use crate::dotool_virtual_keyboard::type_edit_with_dotool;
use crate::error::ImeError;
use crate::ime_config::{ImeConfig, TypingConfig};
use crate::input_method::indicator_phase::{IndicatorFrame, IndicatorPhaseTracker, PhaseTiming};
use crate::input_method::level_bars::LevelBars;
use crate::input_method::wayland_input_method_connection::InputMethodConnection;
use crate::modifier_guard::ModifierGuard;

const LONGEST_IDLE_WAIT: Duration = Duration::from_secs(3600);

pub async fn run_dictation_bus_client(
    connection: InputMethodConnection,
    mut mic_clicks: UnboundedReceiver<()>,
    config: ImeConfig,
) -> Result<(), ImeError> {
    let bus = zbus::Connection::session().await?;
    let proxy = DictationProxy::builder(&bus)
        .cache_properties(zbus::CacheProperties::No)
        .build()
        .await?;
    let owner_watch = proxy.inner().clone();
    let mut owner_changes = owner_watch.receive_owner_changed().await?;
    let mut phase_changes = proxy.receive_phase_changed().await?;
    let mut level_batches = proxy.receive_levels().await?;
    let mut decoding_changes = proxy.receive_decoding_changed().await?;
    let mut transcript_edits = proxy.receive_transcript_edited().await?;
    let mut session_endings = proxy.receive_session_ended().await?;
    let mut session_failures = proxy.receive_session_failed().await?;
    let mut focus_changes = connection.focus_changes();

    let mut client = DictationBusClient::new(connection, proxy, &config);
    client.refresh_daemon_phase().await;
    client.publish_indicator_frame(Instant::now());

    loop {
        let wake_at = client.next_wake_at();
        tokio::select! {
            Some(owner) = owner_changes.next() => match owner {
                Some(_) => client.refresh_daemon_phase().await,
                None => client.daemon_went_away(),
            },
            Some(change) = phase_changes.next() => {
                if let Ok(arguments) = change.args() {
                    client.show_phase(arguments.phase);
                }
            }
            Some(levels) = level_batches.next() => {
                if let Ok(arguments) = levels.args() {
                    client.push_levels(&arguments.frames);
                }
            }
            Some(decoding) = decoding_changes.next() => {
                if let Ok(arguments) = decoding.args() {
                    client.set_decoding(arguments.active);
                }
            }
            Some(edit) = transcript_edits.next() => {
                if let Ok(arguments) = edit.args() {
                    client.type_edit(arguments.edit).await;
                }
            }
            Some(ending) = session_endings.next() => {
                if let Ok(arguments) = ending.args() {
                    client.finish_session(arguments.outcome).await;
                }
            }
            Some(failure) = session_failures.next() => {
                if let Ok(arguments) = failure.args() {
                    client.session_failed(arguments.failure);
                }
            }
            Ok(()) = focus_changes.changed() => client.focus_changed().await,
            Some(()) = mic_clicks.recv() => client.toggle_from_mic_button().await,
            _ = tokio::time::sleep_until(wake_at) => {}
        }
        client.publish_indicator_frame(Instant::now());
    }
}

struct DictationBusClient {
    connection: InputMethodConnection,
    proxy: DictationProxy<'static>,
    session: Option<DictationTypingSession>,
    modifiers_released_since_start: bool,
    typing: TypingConfig,
    tracker: IndicatorPhaseTracker,
    level_bars: LevelBars,
    published_frame: Option<IndicatorFrame>,
}

impl DictationBusClient {
    fn new(
        connection: InputMethodConnection,
        proxy: DictationProxy<'static>,
        config: &ImeConfig,
    ) -> Self {
        let tracker = IndicatorPhaseTracker::new(
            PhaseTiming::from_popup_config(&config.popup),
            connection.focus(),
        );
        Self {
            connection,
            proxy,
            session: None,
            modifiers_released_since_start: false,
            typing: config.typing.clone(),
            tracker,
            level_bars: LevelBars::from_config(&config.popup.level_meter),
            published_frame: None,
        }
    }

    async fn refresh_daemon_phase(&mut self) {
        match self.proxy.current_phase().await {
            Ok(phase) => self.show_phase(phase),
            Err(error) => {
                tracing::warn!(%error, "No notulist daemon on the session bus; start it with `notulist daemon`");
                self.daemon_went_away();
            }
        }
    }

    fn daemon_went_away(&mut self) {
        self.session = None;
        self.tracker.set_daemon_phase(None);
    }

    fn show_phase(&mut self, phase: DictationPhase) {
        tracing::info!(phase = phase.label(), "Daemon phase");
        if phase == DictationPhase::Failed {
            tracing::warn!(
                "The notulist daemon is in the failed phase; its own log names the cause (for example a missing model or microphone)"
            );
        }
        if phase == DictationPhase::Listening {
            self.level_bars.clear();
            if self.session.is_none() {
                self.begin_session();
            }
        }
        self.tracker.set_daemon_phase(Some(phase));
    }

    fn begin_session(&mut self) -> &mut DictationTypingSession {
        let session =
            DictationTypingSession::begin(self.connection.focus(), self.typing.fall_back_to_dotool);
        if session.refuses_input() {
            tracing::info!("Focused field is a password field; dictation will not type into it");
        } else if session.types_with_virtual_keyboard() {
            tracing::info!("No text-input field is focused; typing through dotool");
        } else if session.discards_text() {
            tracing::info!(
                "No text-input field is focused and dotool fallback is off; dictation output is discarded"
            );
        }
        self.modifiers_released_since_start = false;
        self.session.insert(session)
    }

    fn push_levels(&mut self, frames: &[LevelFrame]) {
        self.level_bars.push_frames(frames);
        self.tracker.record_levels(frames, Instant::now());
    }

    fn set_decoding(&mut self, active: bool) {
        self.tracker.set_decoding(active);
    }

    fn session_failed(&mut self, failure: DictationFailure) {
        tracing::warn!(?failure, "Dictation session failed");
        self.tracker.record_session_failure();
    }

    fn session_types_with_keyboard(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(DictationTypingSession::types_with_virtual_keyboard)
    }

    async fn type_edit(&mut self, edit: TranscriptEdit) {
        self.tracker.record_edit(&edit, Instant::now());
        if self.session.is_none() {
            self.begin_session();
        }
        if self.session_types_with_keyboard() && !self.virtual_keyboard_may_type().await {
            tracing::info!(
                "A modifier key is held; stopping dictation instead of typing a shortcut"
            );
            self.request_stop().await;
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let action = session.apply(&edit);
        self.perform(action).await;
    }

    async fn finish_session(&mut self, outcome: SessionOutcome) {
        self.tracker.record_session_end(outcome, Instant::now());
        if let Some(session) = self.session.take() {
            self.perform(session.finish(outcome)).await;
        }
    }

    async fn perform(&self, action: TypingAction) {
        match action {
            TypingAction::Nothing => {}
            TypingAction::SendBatch { field, batch } => {
                if !self.connection.send_batch(field, batch) {
                    tracing::debug!("The dictated text field is no longer focused");
                }
            }
            TypingAction::TypeWithVirtualKeyboard(edit) => {
                if let Err(error) = type_edit_with_dotool(&edit).await {
                    tracing::warn!("{error}");
                }
            }
        }
    }

    async fn virtual_keyboard_may_type(&mut self) -> bool {
        let mut guard = ModifierGuard::new();
        if !guard.any_modifier_held() {
            self.modifiers_released_since_start = true;
            return true;
        }
        if self.modifiers_released_since_start {
            return false;
        }
        let timeout = Duration::from_millis(self.typing.wait_for_modifier_release_ms);
        let released = guard.wait_for_release(timeout).await.is_ok();
        self.modifiers_released_since_start = released;
        released
    }

    async fn focus_changed(&mut self) {
        let focus = self.connection.focus();
        self.tracker.set_focus(focus);
        let lost = self
            .session
            .as_ref()
            .is_some_and(|session| session.lost_its_field(focus));
        if lost {
            tracing::info!("Text field lost focus; stopping dictation");
            self.request_stop().await;
        }
    }

    async fn request_stop(&self) {
        if let Err(error) = self.proxy.stop().await {
            tracing::warn!("Could not stop dictation: {error}");
        }
    }

    async fn toggle_from_mic_button(&self) {
        if let Err(error) = self.proxy.toggle().await {
            tracing::warn!("Could not toggle dictation from the mic button: {error}");
        }
    }

    fn next_wake_at(&self) -> tokio::time::Instant {
        let now = Instant::now();
        let expiry = self
            .tracker
            .next_expiry(now)
            .unwrap_or(now + LONGEST_IDLE_WAIT);
        tokio::time::Instant::from_std(expiry)
    }

    fn publish_indicator_frame(&mut self, now: Instant) {
        let frame = IndicatorFrame {
            phase: self.tracker.current(now),
            level_bar_heights: self.level_bars.heights(),
        };
        if self.published_frame.as_ref() == Some(&frame) {
            return;
        }
        self.connection.show_indicator_frame(frame.clone());
        self.published_frame = Some(frame);
    }
}
