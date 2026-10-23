use std::time::{Duration, Instant};

use notulist_client::{DictationPhase, LevelFrame, SessionOutcome, TranscriptEdit};

use super::TextFieldFocus;
use crate::ime_config::PopupConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorPhase {
    Hidden,
    Offline,
    LoadingModel,
    Ready,
    Failed,
    Waiting,
    Hearing,
    Decoding,
    Correcting,
    Finishing,
    Committed,
    Cancelled,
    Password,
}

impl IndicatorPhase {
    pub const ALL: [IndicatorPhase; 13] = [
        IndicatorPhase::Hidden,
        IndicatorPhase::Offline,
        IndicatorPhase::LoadingModel,
        IndicatorPhase::Ready,
        IndicatorPhase::Failed,
        IndicatorPhase::Waiting,
        IndicatorPhase::Hearing,
        IndicatorPhase::Decoding,
        IndicatorPhase::Correcting,
        IndicatorPhase::Finishing,
        IndicatorPhase::Committed,
        IndicatorPhase::Cancelled,
        IndicatorPhase::Password,
    ];

    pub fn is_animated(self) -> bool {
        matches!(
            self,
            IndicatorPhase::LoadingModel | IndicatorPhase::Finishing
        )
    }

    pub fn accepts_mic_click(self) -> bool {
        matches!(
            self,
            IndicatorPhase::Ready
                | IndicatorPhase::Failed
                | IndicatorPhase::Waiting
                | IndicatorPhase::Hearing
                | IndicatorPhase::Decoding
                | IndicatorPhase::Correcting
                | IndicatorPhase::Finishing
                | IndicatorPhase::Committed
                | IndicatorPhase::Cancelled
        )
    }

    pub fn is_idle(self) -> bool {
        matches!(self, IndicatorPhase::Hidden | IndicatorPhase::Ready)
    }

    pub fn shows_level_bars(self) -> bool {
        matches!(
            self,
            IndicatorPhase::Waiting
                | IndicatorPhase::Hearing
                | IndicatorPhase::Decoding
                | IndicatorPhase::Correcting
        )
    }

    pub fn shows_flat_level_bars(self) -> bool {
        self == IndicatorPhase::Waiting
    }

    pub fn shows_spinner(self) -> bool {
        matches!(
            self,
            IndicatorPhase::LoadingModel | IndicatorPhase::Finishing
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndicatorFrame {
    pub phase: IndicatorPhase,
    pub level_bar_heights: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhaseTiming {
    pub speech_threshold_dbfs: f64,
    pub speech_hold: Duration,
    pub correction_flash: Duration,
    pub outcome_flash: Duration,
}

impl PhaseTiming {
    pub fn from_popup_config(popup: &PopupConfig) -> Self {
        Self {
            speech_threshold_dbfs: popup.speech_detection.speech_loudness_threshold_dbfs,
            speech_hold: Duration::from_millis(popup.speech_detection.keep_hearing_after_speech_ms),
            correction_flash: Duration::from_millis(popup.flashes.correction_flash_ms),
            outcome_flash: Duration::from_millis(popup.flashes.session_outcome_flash_ms),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct OutcomeFlash {
    outcome: SessionOutcome,
    at: Instant,
}

#[derive(Debug, Clone)]
pub struct IndicatorPhaseTracker {
    timing: PhaseTiming,
    daemon_phase: Option<DictationPhase>,
    focus: TextFieldFocus,
    last_speech_at: Option<Instant>,
    decoding: bool,
    last_correction_at: Option<Instant>,
    last_outcome: Option<OutcomeFlash>,
}

impl IndicatorPhaseTracker {
    pub fn new(timing: PhaseTiming, focus: TextFieldFocus) -> Self {
        Self {
            timing,
            daemon_phase: None,
            focus,
            last_speech_at: None,
            decoding: false,
            last_correction_at: None,
            last_outcome: None,
        }
    }

    pub fn set_daemon_phase(&mut self, phase: Option<DictationPhase>) {
        if phase == Some(DictationPhase::Listening) {
            self.last_speech_at = None;
            self.decoding = false;
        }
        self.daemon_phase = phase;
    }

    pub fn set_focus(&mut self, focus: TextFieldFocus) {
        self.focus = focus;
    }

    pub fn set_decoding(&mut self, active: bool) {
        self.decoding = active;
    }

    pub fn record_levels(&mut self, frames: &[LevelFrame], now: Instant) {
        let threshold = self.timing.speech_threshold_dbfs;
        if frames.iter().any(|frame| frame.peak_dbfs >= threshold) {
            self.last_speech_at = Some(now);
        }
    }

    pub fn record_edit(&mut self, edit: &TranscriptEdit, now: Instant) {
        if edit.delete_chars > 0 {
            self.last_correction_at = Some(now);
        }
    }

    pub fn record_session_end(&mut self, outcome: SessionOutcome, now: Instant) {
        self.last_outcome = Some(OutcomeFlash { outcome, at: now });
    }

    pub fn record_session_failure(&mut self) {
        self.last_speech_at = None;
        self.decoding = false;
    }

    pub fn current(&self, now: Instant) -> IndicatorPhase {
        match self.focus {
            TextFieldFocus::Focused {
                sensitive: true, ..
            } => IndicatorPhase::Password,
            TextFieldFocus::Focused { .. } => self.phase_on_text_field(now),
            TextFieldFocus::Connecting
            | TextFieldFocus::Unfocused
            | TextFieldFocus::Unavailable => IndicatorPhase::Hidden,
        }
    }

    pub fn next_expiry(&self, now: Instant) -> Option<Instant> {
        [
            expiry_after(self.last_speech_at, self.timing.speech_hold, now),
            expiry_after(self.last_correction_at, self.timing.correction_flash, now),
            expiry_after(
                self.last_outcome.map(|flash| flash.at),
                self.timing.outcome_flash,
                now,
            ),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn phase_on_text_field(&self, now: Instant) -> IndicatorPhase {
        match self.daemon_phase {
            None => IndicatorPhase::Offline,
            Some(DictationPhase::LoadingModel) => IndicatorPhase::LoadingModel,
            Some(DictationPhase::Failed) => IndicatorPhase::Failed,
            Some(phase) => self.session_phase(phase, now),
        }
    }

    fn session_phase(&self, daemon: DictationPhase, now: Instant) -> IndicatorPhase {
        if let Some(outcome) = self.outcome_flash_at(now) {
            return outcome;
        }
        if within(self.last_correction_at, self.timing.correction_flash, now) {
            return IndicatorPhase::Correcting;
        }
        if self.decoding {
            return IndicatorPhase::Decoding;
        }
        match daemon {
            DictationPhase::Listening
                if within(self.last_speech_at, self.timing.speech_hold, now) =>
            {
                IndicatorPhase::Hearing
            }
            DictationPhase::Listening => IndicatorPhase::Waiting,
            DictationPhase::Finishing => IndicatorPhase::Finishing,
            DictationPhase::Idle | DictationPhase::LoadingModel | DictationPhase::Failed => {
                IndicatorPhase::Ready
            }
        }
    }

    fn outcome_flash_at(&self, now: Instant) -> Option<IndicatorPhase> {
        self.last_outcome
            .filter(|flash| within(Some(flash.at), self.timing.outcome_flash, now))
            .map(|flash| match flash.outcome {
                SessionOutcome::Committed => IndicatorPhase::Committed,
                SessionOutcome::Cancelled => IndicatorPhase::Cancelled,
            })
    }
}

fn within(since: Option<Instant>, window: Duration, now: Instant) -> bool {
    since.is_some_and(|at| at <= now && now < at + window)
}

fn expiry_after(since: Option<Instant>, window: Duration, now: Instant) -> Option<Instant> {
    since.map(|at| at + window).filter(|end| *end > now)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIELD: TextFieldFocus = TextFieldFocus::Focused {
        field: 1,
        sensitive: false,
    };

    fn timing() -> PhaseTiming {
        PhaseTiming {
            speech_threshold_dbfs: -40.0,
            speech_hold: Duration::from_millis(300),
            correction_flash: Duration::from_millis(400),
            outcome_flash: Duration::from_millis(800),
        }
    }

    fn tracker_on(focus: TextFieldFocus) -> IndicatorPhaseTracker {
        IndicatorPhaseTracker::new(timing(), focus)
    }

    fn loud_frame() -> LevelFrame {
        LevelFrame {
            peak_dbfs: -10.0,
            ..LevelFrame::default()
        }
    }

    fn quiet_frame() -> LevelFrame {
        LevelFrame {
            peak_dbfs: -90.0,
            ..LevelFrame::default()
        }
    }

    fn at(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    #[test]
    fn hidden_without_a_focused_field() {
        let start = Instant::now();
        [
            TextFieldFocus::Connecting,
            TextFieldFocus::Unfocused,
            TextFieldFocus::Unavailable,
        ]
        .into_iter()
        .map(tracker_on)
        .for_each(|mut tracker| {
            tracker.set_daemon_phase(Some(DictationPhase::Listening));
            assert_eq!(tracker.current(start), IndicatorPhase::Hidden);
        });
    }

    #[test]
    fn password_fields_hide_the_dictation_state() {
        let start = Instant::now();
        let mut tracker = tracker_on(TextFieldFocus::Focused {
            field: 2,
            sensitive: true,
        });
        tracker.set_daemon_phase(Some(DictationPhase::Listening));
        assert_eq!(tracker.current(start), IndicatorPhase::Password);
        tracker.set_daemon_phase(None);
        assert_eq!(tracker.current(start), IndicatorPhase::Password);
    }

    #[test]
    fn missing_daemon_is_offline() {
        let tracker = tracker_on(FIELD);
        assert_eq!(tracker.current(Instant::now()), IndicatorPhase::Offline);
    }

    #[test]
    fn loading_and_failure_precede_everything_else() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::LoadingModel));
        tracker.set_decoding(true);
        assert_eq!(tracker.current(start), IndicatorPhase::LoadingModel);
        tracker.set_daemon_phase(Some(DictationPhase::Failed));
        assert_eq!(tracker.current(start), IndicatorPhase::Failed);
    }

    #[test]
    fn idle_field_is_ready_and_finishing_is_finishing() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Idle));
        assert_eq!(tracker.current(start), IndicatorPhase::Ready);
        tracker.set_daemon_phase(Some(DictationPhase::Finishing));
        assert_eq!(tracker.current(start), IndicatorPhase::Finishing);
    }

    #[test]
    fn listening_waits_until_speech_then_hears_for_the_hold() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Listening));
        assert_eq!(tracker.current(start), IndicatorPhase::Waiting);
        tracker.record_levels(&[loud_frame()], start);
        assert_eq!(tracker.current(at(start, 299)), IndicatorPhase::Hearing);
        assert_eq!(tracker.current(at(start, 300)), IndicatorPhase::Waiting);
    }

    #[test]
    fn quiet_frames_do_not_count_as_speech() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Listening));
        tracker.record_levels(&[quiet_frame(), quiet_frame()], start);
        assert_eq!(tracker.current(start), IndicatorPhase::Waiting);
    }

    #[test]
    fn decoding_takes_over_while_listening_and_finishing() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Listening));
        tracker.record_levels(&[loud_frame()], start);
        tracker.set_decoding(true);
        assert_eq!(tracker.current(start), IndicatorPhase::Decoding);
        tracker.set_decoding(false);
        assert_eq!(tracker.current(start), IndicatorPhase::Hearing);
        tracker.set_daemon_phase(Some(DictationPhase::Finishing));
        tracker.set_decoding(true);
        assert_eq!(tracker.current(start), IndicatorPhase::Decoding);
    }

    #[test]
    fn corrections_flash_only_when_text_was_deleted() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Listening));
        tracker.record_edit(&TranscriptEdit::append("more"), start);
        assert_eq!(tracker.current(start), IndicatorPhase::Waiting);
        let correction = TranscriptEdit {
            delete_chars: 3,
            insert: "less".into(),
        };
        tracker.record_edit(&correction, start);
        assert_eq!(tracker.current(at(start, 399)), IndicatorPhase::Correcting);
        assert_eq!(tracker.current(at(start, 400)), IndicatorPhase::Waiting);
    }

    #[test]
    fn correction_flash_beats_decoding() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Listening));
        tracker.set_decoding(true);
        let correction = TranscriptEdit {
            delete_chars: 1,
            insert: "x".into(),
        };
        tracker.record_edit(&correction, start);
        assert_eq!(tracker.current(start), IndicatorPhase::Correcting);
    }

    #[test]
    fn session_outcome_flashes_over_the_ready_state() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Idle));
        tracker.record_session_end(SessionOutcome::Committed, start);
        assert_eq!(tracker.current(at(start, 799)), IndicatorPhase::Committed);
        assert_eq!(tracker.current(at(start, 800)), IndicatorPhase::Ready);

        tracker.record_session_end(SessionOutcome::Cancelled, at(start, 1000));
        assert_eq!(tracker.current(at(start, 1000)), IndicatorPhase::Cancelled);
    }

    #[test]
    fn outcome_flash_beats_a_correction_flash() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Idle));
        let correction = TranscriptEdit {
            delete_chars: 2,
            insert: String::new(),
        };
        tracker.record_edit(&correction, start);
        tracker.record_session_end(SessionOutcome::Committed, start);
        assert_eq!(tracker.current(start), IndicatorPhase::Committed);
    }

    #[test]
    fn failure_beats_a_session_outcome_flash() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.record_session_end(SessionOutcome::Cancelled, start);
        tracker.set_daemon_phase(Some(DictationPhase::Failed));
        assert_eq!(tracker.current(start), IndicatorPhase::Failed);
    }

    #[test]
    fn session_failure_stops_hearing_and_decoding() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Listening));
        tracker.record_levels(&[loud_frame()], start);
        tracker.set_decoding(true);
        tracker.record_session_failure();
        assert_eq!(tracker.current(start), IndicatorPhase::Waiting);
    }

    #[test]
    fn a_new_listening_phase_forgets_the_previous_speech() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Listening));
        tracker.record_levels(&[loud_frame()], start);
        tracker.set_daemon_phase(Some(DictationPhase::Idle));
        tracker.set_daemon_phase(Some(DictationPhase::Listening));
        assert_eq!(tracker.current(start), IndicatorPhase::Waiting);
    }

    #[test]
    fn leaving_the_field_hides_everything() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Listening));
        tracker.set_decoding(true);
        tracker.set_focus(TextFieldFocus::Unfocused);
        assert_eq!(tracker.current(start), IndicatorPhase::Hidden);
    }

    #[test]
    fn next_expiry_is_the_earliest_pending_flash_or_hold() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.set_daemon_phase(Some(DictationPhase::Listening));
        assert_eq!(tracker.next_expiry(start), None);

        tracker.record_levels(&[loud_frame()], start);
        assert_eq!(tracker.next_expiry(start), Some(at(start, 300)));

        let correction = TranscriptEdit {
            delete_chars: 1,
            insert: String::new(),
        };
        tracker.record_edit(&correction, at(start, 100));
        assert_eq!(tracker.next_expiry(start), Some(at(start, 300)));
        assert_eq!(tracker.next_expiry(at(start, 300)), Some(at(start, 500)));
        assert_eq!(tracker.next_expiry(at(start, 500)), None);
    }

    #[test]
    fn expired_outcome_flash_schedules_nothing() {
        let start = Instant::now();
        let mut tracker = tracker_on(FIELD);
        tracker.record_session_end(SessionOutcome::Committed, start);
        assert_eq!(tracker.next_expiry(start), Some(at(start, 800)));
        assert_eq!(tracker.next_expiry(at(start, 800)), None);
    }

    #[test]
    fn only_loading_and_finishing_animate() {
        let animated: Vec<IndicatorPhase> = IndicatorPhase::ALL
            .iter()
            .copied()
            .filter(|phase| phase.is_animated())
            .collect();
        assert_eq!(
            animated,
            vec![IndicatorPhase::LoadingModel, IndicatorPhase::Finishing]
        );
    }

    #[test]
    fn mic_button_accepts_clicks_only_when_the_daemon_can_act() {
        assert!(IndicatorPhase::Ready.accepts_mic_click());
        assert!(IndicatorPhase::Failed.accepts_mic_click());
        assert!(IndicatorPhase::Hearing.accepts_mic_click());
        assert!(IndicatorPhase::Decoding.accepts_mic_click());
        assert!(!IndicatorPhase::Password.accepts_mic_click());
        assert!(!IndicatorPhase::Offline.accepts_mic_click());
        assert!(!IndicatorPhase::LoadingModel.accepts_mic_click());
        assert!(!IndicatorPhase::Hidden.accepts_mic_click());
    }

    #[test]
    fn level_bars_show_only_for_listening_like_phases() {
        let bar_phases: Vec<IndicatorPhase> = IndicatorPhase::ALL
            .iter()
            .copied()
            .filter(|phase| phase.shows_level_bars())
            .collect();
        assert_eq!(
            bar_phases,
            vec![
                IndicatorPhase::Waiting,
                IndicatorPhase::Hearing,
                IndicatorPhase::Decoding,
                IndicatorPhase::Correcting
            ]
        );
    }

    #[test]
    fn only_hidden_and_ready_are_idle() {
        let idle: Vec<IndicatorPhase> = IndicatorPhase::ALL
            .iter()
            .copied()
            .filter(|phase| phase.is_idle())
            .collect();
        assert_eq!(idle, vec![IndicatorPhase::Hidden, IndicatorPhase::Ready]);
    }
}
