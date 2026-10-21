//! Pure mapping from daemon state to what the indicator pill draws.

/// What the pill renders, independent of drawing code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorVisual {
    /// No focused field, a password field, or the feature is off.
    Off,
    /// Daemon is idle; the pill is shown but dim.
    Idle,
    /// Recording or streaming; waveform animates.
    Listening,
    /// Transcribing; spinner animates.
    Processing,
}

/// Map a daemon state name and the focused field onto a visual.
///
/// The `daemon_state` values are the strings fed to `update_state`:
/// `idle`, `recording`, `streaming`, `transcribing`. Anything else
/// (including the older `stopped`) falls back to [`IndicatorVisual::Idle`].
pub fn visual_state(daemon_state: &str, focused: bool, sensitive: bool) -> IndicatorVisual {
    if !focused || sensitive {
        return IndicatorVisual::Off;
    }
    match daemon_state {
        "recording" | "streaming" => IndicatorVisual::Listening,
        "transcribing" => IndicatorVisual::Processing,
        _ => IndicatorVisual::Idle,
    }
}

/// The signal to send the daemon when the mic button is clicked.
///
/// Idle starts a recording (`SIGUSR1`); listening/processing stops one
/// (`SIGUSR2`). `Off` has no field to dictate into, so no signal.
pub fn mic_signal(visual: IndicatorVisual) -> Option<libc::c_int> {
    match visual {
        IndicatorVisual::Idle => Some(libc::SIGUSR1),
        IndicatorVisual::Listening | IndicatorVisual::Processing => Some(libc::SIGUSR2),
        IndicatorVisual::Off => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_when_unfocused() {
        assert_eq!(visual_state("idle", false, false), IndicatorVisual::Off);
    }

    #[test]
    fn off_for_password_fields_ignores_daemon_state() {
        assert_eq!(visual_state("recording", true, true), IndicatorVisual::Off);
    }

    #[test]
    fn idle_maps_daemon_idle_and_unknown() {
        assert_eq!(visual_state("idle", true, false), IndicatorVisual::Idle);
        assert_eq!(visual_state("stopped", true, false), IndicatorVisual::Idle);
        assert_eq!(visual_state("bogus", true, false), IndicatorVisual::Idle);
    }

    #[test]
    fn listening_maps_recording_and_streaming() {
        assert_eq!(visual_state("recording", true, false), IndicatorVisual::Listening);
        assert_eq!(visual_state("streaming", true, false), IndicatorVisual::Listening);
    }

    #[test]
    fn processing_maps_transcribing() {
        assert_eq!(visual_state("transcribing", true, false), IndicatorVisual::Processing);
    }

    #[test]
    fn mic_signal_matches_record_toggle_semantics() {
        assert_eq!(mic_signal(IndicatorVisual::Idle), Some(libc::SIGUSR1));
        assert_eq!(mic_signal(IndicatorVisual::Listening), Some(libc::SIGUSR2));
        assert_eq!(mic_signal(IndicatorVisual::Processing), Some(libc::SIGUSR2));
        assert_eq!(mic_signal(IndicatorVisual::Off), None);
    }
}