use std::collections::HashSet;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::Duration;

use evdev::{BusType, Device, EventStream, EventSummary, KeyCode};
use futures_util::{stream, TryStreamExt};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::config::ShortcutChords;
use crate::dbus_dictation_service::DictationCommand;
use crate::error::ShortcutError;

const INPUT_DEVICE_DIRECTORY: &str = "/dev/input";
const KEYBOARD_RESCAN_INTERVAL: Duration = Duration::from_secs(5);
const KEYBOARD_EVENT_QUEUE_DEPTH: usize = 64;
const VIRTUAL_DEVICE_NAME_MARKERS: [&str; 3] = ["dotool", "ydotool", "virtual"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutAction {
    Start,
    Stop,
    Toggle,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChordTransition {
    Completed,
    Broken,
    Unchanged,
}

#[derive(Debug, Clone)]
struct ChordState {
    keys: Vec<KeyCode>,
    complete: bool,
}

impl ChordState {
    fn new(chord_keys: &[KeyCode]) -> Self {
        let mut keys: Vec<KeyCode> = chord_keys.iter().copied().map(canonical_modifier).collect();
        keys.sort_by_key(|key| key.code());
        keys.dedup();
        Self {
            keys,
            complete: false,
        }
    }

    fn update(&mut self, held_keys: &HashSet<KeyCode>) -> ChordTransition {
        let was_complete = self.complete;
        self.complete = !self.keys.is_empty()
            && self.keys.iter().all(|chord_key| {
                held_keys
                    .iter()
                    .any(|held_key| canonical_modifier(*held_key) == *chord_key)
            });
        match (was_complete, self.complete) {
            (false, true) => ChordTransition::Completed,
            (true, false) => ChordTransition::Broken,
            _ => ChordTransition::Unchanged,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShortcutTracker {
    hold_chord: ChordState,
    toggle_chord: ChordState,
    cancel_chord: ChordState,
    held_keys: HashSet<KeyCode>,
    hold_session_started: bool,
}

impl ShortcutTracker {
    pub fn new(chords: &ShortcutChords) -> Self {
        Self {
            hold_chord: ChordState::new(&chords.hold_keys),
            toggle_chord: ChordState::new(&chords.toggle_keys),
            cancel_chord: ChordState::new(&chords.cancel_keys),
            held_keys: HashSet::new(),
            hold_session_started: false,
        }
    }

    pub fn observe(&mut self, key: KeyCode, pressed: bool) -> Vec<ShortcutAction> {
        if pressed {
            self.held_keys.insert(key);
        } else {
            self.held_keys.remove(&key);
        }
        self.update_chord_states()
    }

    pub fn release_all_keys(&mut self) -> Vec<ShortcutAction> {
        self.held_keys.clear();
        self.update_chord_states()
    }

    fn update_chord_states(&mut self) -> Vec<ShortcutAction> {
        let cancel_transition = self.cancel_chord.update(&self.held_keys);
        let hold_transition = self.hold_chord.update(&self.held_keys);
        let toggle_transition = self.toggle_chord.update(&self.held_keys);
        if cancel_transition == ChordTransition::Completed {
            self.hold_session_started = false;
            return vec![ShortcutAction::Cancel];
        }
        let hold_action = match hold_transition {
            ChordTransition::Completed => {
                self.hold_session_started = true;
                Some(ShortcutAction::Start)
            }
            ChordTransition::Broken if self.hold_session_started => {
                self.hold_session_started = false;
                Some(ShortcutAction::Stop)
            }
            ChordTransition::Broken | ChordTransition::Unchanged => None,
        };
        let toggle_action = match toggle_transition {
            ChordTransition::Completed if !self.hold_chord.complete => Some(ShortcutAction::Toggle),
            ChordTransition::Completed | ChordTransition::Broken | ChordTransition::Unchanged => {
                None
            }
        };
        hold_action.into_iter().chain(toggle_action).collect()
    }
}

fn canonical_modifier(key: KeyCode) -> KeyCode {
    match key {
        KeyCode::KEY_RIGHTMETA => KeyCode::KEY_LEFTMETA,
        KeyCode::KEY_RIGHTCTRL => KeyCode::KEY_LEFTCTRL,
        KeyCode::KEY_RIGHTALT => KeyCode::KEY_LEFTALT,
        KeyCode::KEY_RIGHTSHIFT => KeyCode::KEY_LEFTSHIFT,
        other => other,
    }
}

fn pressed_from_key_value(value: i32) -> Option<bool> {
    match value {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn dictation_command_for(action: ShortcutAction) -> DictationCommand {
    match action {
        ShortcutAction::Start => DictationCommand::Start,
        ShortcutAction::Stop => DictationCommand::Stop,
        ShortcutAction::Toggle => DictationCommand::Toggle,
        ShortcutAction::Cancel => DictationCommand::Cancel,
    }
}

#[derive(Debug, Clone, Copy)]
struct KeyObservation {
    key: KeyCode,
    pressed: bool,
}

enum KeyboardEvent {
    KeyChanged(KeyObservation),
    DeviceClosed(PathBuf),
}

struct OpenedKeyboard {
    path: PathBuf,
    name: String,
    stream: EventStream,
}

struct KeyboardShortcutListener {
    tracker: ShortcutTracker,
    commands: mpsc::Sender<DictationCommand>,
    watched_devices: HashSet<PathBuf>,
    keyboard_events: mpsc::Sender<KeyboardEvent>,
    missing_keyboard_reported: bool,
}

pub async fn listen_for_keyboard_shortcut(
    tracker: ShortcutTracker,
    commands: mpsc::Sender<DictationCommand>,
) {
    tracing::info!(
        hold_chord = ?tracker.hold_chord.keys,
        toggle_chord = ?tracker.toggle_chord.keys,
        cancel_chord = ?tracker.cancel_chord.keys,
        "Listening for the dictation keyboard shortcut"
    );
    let (keyboard_events, mut keyboard_events_rx) = mpsc::channel(KEYBOARD_EVENT_QUEUE_DEPTH);
    let mut listener = KeyboardShortcutListener {
        tracker,
        commands,
        watched_devices: HashSet::new(),
        keyboard_events,
        missing_keyboard_reported: false,
    };
    let mut rescan_timer = tokio::time::interval(KEYBOARD_RESCAN_INTERVAL);
    rescan_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        let flow = tokio::select! {
            _ = rescan_timer.tick() => {
                listener.watch_new_keyboards();
                ControlFlow::Continue(())
            }
            Some(event) = keyboard_events_rx.recv() => listener.handle_keyboard_event(event).await,
        };
        if flow.is_break() {
            return;
        }
    }
}

impl KeyboardShortcutListener {
    async fn handle_keyboard_event(&mut self, event: KeyboardEvent) -> ControlFlow<()> {
        let actions = match event {
            KeyboardEvent::KeyChanged(observation) => {
                self.tracker.observe(observation.key, observation.pressed)
            }
            KeyboardEvent::DeviceClosed(path) => {
                self.watched_devices.remove(&path);
                tracing::info!(path = %path.display(), "Keyboard disappeared");
                self.watch_new_keyboards();
                self.tracker.release_all_keys()
            }
        };
        let commands = &self.commands;
        let delivered = stream::iter(actions.into_iter().map(Ok))
            .try_for_each(|action| async move {
                tracing::debug!(?action, "Keyboard shortcut triggered");
                commands.send(dictation_command_for(action)).await
            })
            .await;
        if delivered.is_err() {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }

    fn watch_new_keyboards(&mut self) {
        match open_unwatched_keyboards(&self.watched_devices) {
            Ok(keyboards) => keyboards
                .into_iter()
                .for_each(|keyboard| self.watch_keyboard(keyboard)),
            Err(error) => tracing::debug!(%error, "Could not scan for keyboards"),
        }
        self.report_missing_keyboards();
    }

    fn watch_keyboard(&mut self, keyboard: OpenedKeyboard) {
        let OpenedKeyboard { path, name, stream } = keyboard;
        tracing::info!(device = name, path = %path.display(), "Watching keyboard for the shortcut");
        self.watched_devices.insert(path.clone());
        tokio::spawn(forward_key_events(
            stream,
            path,
            self.keyboard_events.clone(),
        ));
    }

    fn report_missing_keyboards(&mut self) {
        if !self.watched_devices.is_empty() {
            self.missing_keyboard_reported = false;
            return;
        }
        if self.missing_keyboard_reported {
            return;
        }
        self.missing_keyboard_reported = true;
        tracing::error!(
            "No readable keyboard in {INPUT_DEVICE_DIRECTORY}; the dictation shortcut is inactive until one appears. Add your user to the `input` group, or enable programs.notulist.shortcutListener in the notulist NixOS module, which installs a uaccess udev rule. D-Bus control keeps working."
        );
    }
}

async fn forward_key_events(
    mut stream: EventStream,
    path: PathBuf,
    keyboard_events: mpsc::Sender<KeyboardEvent>,
) {
    while let Ok(event) = stream.next_event().await {
        let EventSummary::Key(_, key, value) = event.destructure() else {
            continue;
        };
        let Some(pressed) = pressed_from_key_value(value) else {
            continue;
        };
        let observation = KeyObservation { key, pressed };
        if keyboard_events
            .send(KeyboardEvent::KeyChanged(observation))
            .await
            .is_err()
        {
            return;
        }
    }
    let _closed = keyboard_events
        .send(KeyboardEvent::DeviceClosed(path))
        .await;
}

fn open_unwatched_keyboards(
    watched_devices: &HashSet<PathBuf>,
) -> Result<Vec<OpenedKeyboard>, ShortcutError> {
    let entries = std::fs::read_dir(INPUT_DEVICE_DIRECTORY)?;
    Ok(entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_event_node(path) && !watched_devices.contains(path))
        .filter_map(open_keyboard_stream)
        .collect())
}

fn open_keyboard_stream(path: PathBuf) -> Option<OpenedKeyboard> {
    let device = Device::open(&path).ok()?;
    if !is_keyboard(&device) {
        return None;
    }
    let name = device.name().unwrap_or("unnamed keyboard").to_owned();
    if is_virtual_input_device(device.input_id().bus_type(), &name) {
        tracing::debug!(device = name, path = %path.display(), "Skipping virtual keyboard");
        return None;
    }
    let stream = device.into_event_stream().ok()?;
    Some(OpenedKeyboard { path, name, stream })
}

fn is_virtual_input_device(bus_type: BusType, name: &str) -> bool {
    let lowercase_name = name.to_lowercase();
    bus_type == BusType::BUS_VIRTUAL
        || VIRTUAL_DEVICE_NAME_MARKERS
            .iter()
            .any(|marker| lowercase_name.contains(marker))
}

fn is_event_node(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("event"))
}

fn is_keyboard(device: &Device) -> bool {
    device.supported_keys().is_some_and(|keys| {
        keys.contains(KeyCode::KEY_A)
            && keys.contains(KeyCode::KEY_Z)
            && keys.contains(KeyCode::KEY_ENTER)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_ACTIONS: Vec<ShortcutAction> = Vec::new();

    const SUPER_V: [KeyCode; 2] = [KeyCode::KEY_LEFTMETA, KeyCode::KEY_V];
    const SUPER_T: [KeyCode; 2] = [KeyCode::KEY_LEFTMETA, KeyCode::KEY_T];
    const SUPER_SHIFT_V: [KeyCode; 3] = [
        KeyCode::KEY_LEFTMETA,
        KeyCode::KEY_LEFTSHIFT,
        KeyCode::KEY_V,
    ];

    fn tracker_with_three_chords(
        hold_keys: &[KeyCode],
        toggle_keys: &[KeyCode],
        cancel_keys: &[KeyCode],
    ) -> ShortcutTracker {
        ShortcutTracker::new(&ShortcutChords {
            hold_keys: hold_keys.to_vec(),
            toggle_keys: toggle_keys.to_vec(),
            cancel_keys: cancel_keys.to_vec(),
        })
    }

    fn tracker_with_chords(hold_keys: &[KeyCode], toggle_keys: &[KeyCode]) -> ShortcutTracker {
        tracker_with_three_chords(hold_keys, toggle_keys, &[])
    }

    fn default_tracker() -> ShortcutTracker {
        tracker_with_three_chords(&SUPER_V, &SUPER_T, &SUPER_SHIFT_V)
    }

    #[test]
    fn cancel_from_idle() {
        let mut tracker = tracker_with_three_chords(&[], &SUPER_T, &SUPER_SHIFT_V);
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_LEFTSHIFT, true);
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, true),
            vec![ShortcutAction::Cancel]
        );
        assert_eq!(tracker.observe(KeyCode::KEY_V, false), NO_ACTIONS);
    }

    #[test]
    fn cancel_suppresses_hold_start_on_same_press() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_LEFTSHIFT, true);
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, true),
            vec![ShortcutAction::Cancel]
        );
        assert_eq!(tracker.observe(KeyCode::KEY_LEFTSHIFT, false), NO_ACTIONS);
        assert_eq!(tracker.observe(KeyCode::KEY_V, false), NO_ACTIONS);
    }

    #[test]
    fn cancel_during_hold_ends_hold_without_stop() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, true),
            vec![ShortcutAction::Start]
        );
        assert_eq!(
            tracker.observe(KeyCode::KEY_RIGHTSHIFT, true),
            vec![ShortcutAction::Cancel]
        );
        assert_eq!(tracker.observe(KeyCode::KEY_RIGHTSHIFT, false), NO_ACTIONS);
        assert_eq!(tracker.observe(KeyCode::KEY_V, false), NO_ACTIONS);
        assert_eq!(tracker.release_all_keys(), NO_ACTIONS);
    }

    #[test]
    fn empty_cancel_chord_leaves_hold_working() {
        let mut tracker = tracker_with_three_chords(&SUPER_V, &SUPER_T, &[]);
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_LEFTSHIFT, true);
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, true),
            vec![ShortcutAction::Start]
        );
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, false),
            vec![ShortcutAction::Stop]
        );
    }

    #[test]
    fn hold_starts_when_chord_completes_and_stops_when_it_breaks() {
        let mut tracker = default_tracker();
        assert_eq!(tracker.observe(KeyCode::KEY_LEFTMETA, true), NO_ACTIONS);
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, true),
            vec![ShortcutAction::Start]
        );
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, false),
            vec![ShortcutAction::Stop]
        );
        assert_eq!(tracker.observe(KeyCode::KEY_LEFTMETA, false), NO_ACTIONS);
    }

    #[test]
    fn hold_stops_when_modifier_released_first() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_V, true);
        assert_eq!(
            tracker.observe(KeyCode::KEY_LEFTMETA, false),
            vec![ShortcutAction::Stop]
        );
        assert_eq!(tracker.observe(KeyCode::KEY_V, false), NO_ACTIONS);
    }

    #[test]
    fn toggle_fires_once_per_completion() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        assert_eq!(
            tracker.observe(KeyCode::KEY_T, true),
            vec![ShortcutAction::Toggle]
        );
        assert_eq!(tracker.observe(KeyCode::KEY_T, false), NO_ACTIONS);
        assert_eq!(
            tracker.observe(KeyCode::KEY_T, true),
            vec![ShortcutAction::Toggle]
        );
    }

    #[test]
    fn repeated_presses_while_complete_do_not_retrigger() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_T, true);
        assert_eq!(tracker.observe(KeyCode::KEY_T, true), NO_ACTIONS);
        assert_eq!(tracker.observe(KeyCode::KEY_LEFTMETA, true), NO_ACTIONS);
        assert_eq!(tracker.observe(KeyCode::KEY_B, true), NO_ACTIONS);
    }

    #[test]
    fn releasing_unrelated_key_does_nothing() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_V, true);
        assert_eq!(tracker.observe(KeyCode::KEY_B, false), NO_ACTIONS);
        assert_eq!(tracker.observe(KeyCode::KEY_LEFTSHIFT, false), NO_ACTIONS);
    }

    #[test]
    fn right_modifier_matches_left_modifier() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_RIGHTMETA, true);
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, true),
            vec![ShortcutAction::Start]
        );
    }

    #[test]
    fn releasing_one_of_two_held_modifiers_keeps_chord() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_RIGHTMETA, true);
        tracker.observe(KeyCode::KEY_V, true);
        assert_eq!(tracker.observe(KeyCode::KEY_LEFTMETA, false), NO_ACTIONS);
        assert_eq!(
            tracker.observe(KeyCode::KEY_RIGHTMETA, false),
            vec![ShortcutAction::Stop]
        );
    }

    #[test]
    fn pressing_t_during_hold_does_not_toggle_without_its_chord() {
        let mut tracker = tracker_with_chords(
            &[KeyCode::KEY_LEFTMETA, KeyCode::KEY_V],
            &[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_T],
        );
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_V, true);
        assert_eq!(tracker.observe(KeyCode::KEY_T, true), NO_ACTIONS);
        assert_eq!(tracker.observe(KeyCode::KEY_T, false), NO_ACTIONS);
    }

    #[test]
    fn toggle_chord_is_ignored_while_hold_chord_is_active() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, true),
            vec![ShortcutAction::Start]
        );
        assert_eq!(tracker.observe(KeyCode::KEY_T, true), NO_ACTIONS);
        assert_eq!(tracker.observe(KeyCode::KEY_T, false), NO_ACTIONS);
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, false),
            vec![ShortcutAction::Stop]
        );
    }

    #[test]
    fn toggle_chord_works_after_hold_chord_is_released() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_V, true);
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, false),
            vec![ShortcutAction::Stop]
        );
        assert_eq!(
            tracker.observe(KeyCode::KEY_T, true),
            vec![ShortcutAction::Toggle]
        );
    }

    #[test]
    fn releasing_hold_key_while_toggle_chord_held_does_not_toggle() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_V, true);
        tracker.observe(KeyCode::KEY_T, true);
        assert_eq!(
            tracker.observe(KeyCode::KEY_V, false),
            vec![ShortcutAction::Stop]
        );
    }

    #[test]
    fn modifier_completing_both_chords_only_starts_hold() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_V, true);
        tracker.observe(KeyCode::KEY_T, true);
        assert_eq!(
            tracker.observe(KeyCode::KEY_LEFTMETA, true),
            vec![ShortcutAction::Start]
        );
    }

    #[test]
    fn virtual_devices_are_recognised_by_bus_or_name() {
        let usb_bus = BusType::BUS_USB;
        assert!(is_virtual_input_device(
            BusType::BUS_VIRTUAL,
            "Some keyboard"
        ));
        assert!(is_virtual_input_device(usb_bus, "dotool keyboard"));
        assert!(is_virtual_input_device(usb_bus, "ydotoold virtual device"));
        assert!(is_virtual_input_device(usb_bus, "Fake VIRTUAL Keys"));
        assert!(!is_virtual_input_device(
            usb_bus,
            "ZSA Technology Labs Moonlander Mark I"
        ));
        assert!(!is_virtual_input_device(
            BusType::BUS_I8042,
            "AT Translated Set 2 keyboard"
        ));
    }

    #[test]
    fn empty_chords_never_trigger() {
        let mut tracker = tracker_with_chords(&[], &[KeyCode::KEY_LEFTMETA, KeyCode::KEY_T]);
        assert_eq!(tracker.observe(KeyCode::KEY_V, true), NO_ACTIONS);
        assert_eq!(tracker.observe(KeyCode::KEY_V, false), NO_ACTIONS);
        let mut disabled_tracker = tracker_with_chords(&[], &[]);
        assert_eq!(disabled_tracker.observe(KeyCode::KEY_A, true), NO_ACTIONS);
        assert_eq!(disabled_tracker.release_all_keys(), NO_ACTIONS);
    }

    #[test]
    fn releasing_all_keys_stops_held_chord() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_V, true);
        assert_eq!(tracker.release_all_keys(), vec![ShortcutAction::Stop]);
        assert_eq!(tracker.release_all_keys(), NO_ACTIONS);
    }

    #[test]
    fn releasing_all_keys_after_toggle_sends_nothing() {
        let mut tracker = default_tracker();
        tracker.observe(KeyCode::KEY_LEFTMETA, true);
        tracker.observe(KeyCode::KEY_T, true);
        assert_eq!(tracker.release_all_keys(), NO_ACTIONS);
    }

    #[test]
    fn autorepeat_value_is_ignored() {
        assert_eq!(pressed_from_key_value(1), Some(true));
        assert_eq!(pressed_from_key_value(0), Some(false));
        assert_eq!(pressed_from_key_value(2), None);
    }
}
