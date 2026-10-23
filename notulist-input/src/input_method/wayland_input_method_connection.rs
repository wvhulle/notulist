use std::sync::Arc;

use calloop::{channel, EventLoop};
use calloop_wayland_source::WaylandSource;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    ContentHint, ContentPurpose,
};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::{
    self, ZwpInputMethodV2,
};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2;

use super::indicator_phase::IndicatorFrame;
use super::indicator_popup::IndicatorPopup;
use super::preedit_commit_tracker::InputMethodBatch;
use super::TextFieldFocus;
use crate::error::ImeError;
use crate::ime_config::PopupConfig;

const LEFT_MOUSE_BUTTON: u32 = 0x110;

pub struct FieldBatch {
    pub field: u32,
    pub batch: InputMethodBatch,
}

pub struct InputMethodConnection {
    batches: channel::Sender<FieldBatch>,
    frames: channel::Sender<IndicatorFrame>,
    focus: watch::Receiver<TextFieldFocus>,
}

impl InputMethodConnection {
    pub fn spawn(popup: PopupConfig, mic_clicks: UnboundedSender<()>) -> Self {
        let (batches, batch_source) = channel::channel();
        let (frames, frame_source) = channel::channel();
        let (focus_sender, focus) = watch::channel(TextFieldFocus::Connecting);
        let focus_sender = Arc::new(focus_sender);
        let thread_focus_sender = Arc::clone(&focus_sender);
        let spawned = std::thread::Builder::new()
            .name("notulist-input-method".to_owned())
            .spawn(move || {
                let outcome = run_input_method_loop(
                    batch_source,
                    frame_source,
                    &thread_focus_sender,
                    &popup,
                    mic_clicks,
                );
                if let Err(error) = outcome {
                    tracing::warn!("Input method connection unavailable: {error}");
                    publish_focus(&thread_focus_sender, TextFieldFocus::Unavailable);
                }
            });
        if let Err(error) = spawned {
            tracing::warn!("Could not start the input method thread: {error}");
            publish_focus(&focus_sender, TextFieldFocus::Unavailable);
        }
        Self {
            batches,
            frames,
            focus,
        }
    }

    pub fn focus(&self) -> TextFieldFocus {
        *self.focus.borrow()
    }

    pub fn focus_changes(&self) -> watch::Receiver<TextFieldFocus> {
        self.focus.clone()
    }

    pub fn send_batch(&self, field: u32, batch: InputMethodBatch) -> bool {
        self.focus().writable_field() == Some(field)
            && self.batches.send(FieldBatch { field, batch }).is_ok()
    }

    pub fn show_indicator_frame(&self, frame: IndicatorFrame) {
        let _ = self.frames.send(frame);
    }
}

#[derive(Default)]
struct PendingFieldState {
    focused: bool,
    activated_since_done: bool,
    sensitive: bool,
}

pub struct InputMethodState {
    input_method: ZwpInputMethodV2,
    queue_handle: QueueHandle<InputMethodState>,
    done_events: u32,
    pending: PendingFieldState,
    activated_fields: u32,
    focus: TextFieldFocus,
    focus_sender: Arc<watch::Sender<TextFieldFocus>>,
    popup: IndicatorPopup,
}

impl InputMethodState {
    fn apply_field_batch(&self, field_batch: FieldBatch) {
        let FieldBatch { field, batch } = field_batch;
        if self.focus.writable_field() != Some(field) {
            tracing::debug!("Dropping input method batch for a text field that lost focus");
            return;
        }
        if batch.delete_before_bytes > 0 {
            self.input_method
                .delete_surrounding_text(batch.delete_before_bytes, 0);
        }
        if !batch.commit_text.is_empty() {
            self.input_method.commit_string(batch.commit_text);
        }
        let preedit_cursor = i32::try_from(batch.preedit_text.len()).unwrap_or(i32::MAX);
        self.input_method
            .set_preedit_string(batch.preedit_text, preedit_cursor, preedit_cursor);
        self.input_method.commit(self.done_events);
    }

    fn show_indicator_frame(&mut self, frame: IndicatorFrame) {
        self.popup
            .show_indicator_frame(frame, &self.input_method, &self.queue_handle);
    }

    fn settle_focus(&mut self, focus: TextFieldFocus) {
        self.focus = focus;
        publish_focus(&self.focus_sender, focus);
        self.popup
            .follow_focus(focus, &self.input_method, &self.queue_handle);
    }

    fn settle_done_event(&mut self) {
        self.done_events = self.done_events.wrapping_add(1);
        if std::mem::take(&mut self.pending.activated_since_done) {
            self.activated_fields = self.activated_fields.wrapping_add(1);
        }
        let focus = match self.pending.focused {
            true => TextFieldFocus::Focused {
                field: self.activated_fields,
                sensitive: self.pending.sensitive,
            },
            false => TextFieldFocus::Unfocused,
        };
        self.settle_focus(focus);
    }
}

fn publish_focus(sender: &watch::Sender<TextFieldFocus>, focus: TextFieldFocus) {
    sender.send_if_modified(|current| {
        let changed = *current != focus;
        *current = focus;
        changed
    });
}

pub fn is_sensitive_content(hint: ContentHint, purpose: ContentPurpose) -> bool {
    hint.intersects(ContentHint::SensitiveData | ContentHint::HiddenText)
        || matches!(purpose, ContentPurpose::Password | ContentPurpose::Pin)
}

fn run_input_method_loop(
    batch_source: channel::Channel<FieldBatch>,
    frame_source: channel::Channel<IndicatorFrame>,
    focus_sender: &Arc<watch::Sender<TextFieldFocus>>,
    popup_config: &PopupConfig,
    mic_clicks: UnboundedSender<()>,
) -> Result<(), ImeError> {
    let connection = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init::<InputMethodState>(&connection)?;
    let queue_handle = event_queue.handle();
    let seat: WlSeat = globals.bind(&queue_handle, 1..=7, ())?;
    let manager: ZwpInputMethodManagerV2 = globals.bind(&queue_handle, 1..=1, ())?;
    let compositor: WlCompositor = globals.bind(&queue_handle, 4..=6, ())?;
    let shm: WlShm = globals.bind(&queue_handle, 1..=1, ())?;
    let input_method = manager.get_input_method(&seat, &queue_handle, ());

    let mut event_loop: EventLoop<InputMethodState> = EventLoop::try_new()?;
    WaylandSource::new(connection, event_queue).insert(event_loop.handle())?;
    event_loop
        .handle()
        .insert_source(batch_source, |event, _, state| {
            if let channel::Event::Msg(field_batch) = event {
                state.apply_field_batch(field_batch);
            }
        })?;
    event_loop
        .handle()
        .insert_source(frame_source, |event, _, state| {
            if let channel::Event::Msg(frame) = event {
                state.show_indicator_frame(frame);
            }
        })?;

    let mut state = InputMethodState {
        input_method,
        queue_handle: queue_handle.clone(),
        done_events: 0,
        pending: PendingFieldState::default(),
        activated_fields: 0,
        focus: TextFieldFocus::Unfocused,
        focus_sender: Arc::clone(focus_sender),
        popup: IndicatorPopup::new(compositor, shm, popup_config, mic_clicks),
    };
    publish_focus(focus_sender, TextFieldFocus::Unfocused);
    event_loop.run(None, &mut state, |_| {})?;
    Ok(())
}

impl Dispatch<ZwpInputMethodV2, ()> for InputMethodState {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::Activate => {
                state.pending = PendingFieldState {
                    focused: true,
                    activated_since_done: true,
                    sensitive: false,
                };
            }
            zwp_input_method_v2::Event::Deactivate => state.pending.focused = false,
            zwp_input_method_v2::Event::ContentType { hint, purpose } => {
                let hint = match hint {
                    WEnum::Value(hint) => hint,
                    WEnum::Unknown(bits) => ContentHint::from_bits_truncate(bits),
                };
                let purpose = match purpose {
                    WEnum::Value(purpose) => purpose,
                    WEnum::Unknown(_) => ContentPurpose::Normal,
                };
                state.pending.sensitive = is_sensitive_content(hint, purpose);
            }
            zwp_input_method_v2::Event::Done => state.settle_done_event(),
            zwp_input_method_v2::Event::Unavailable => {
                tracing::warn!("Another input method already serves this seat");
                state.settle_focus(TextFieldFocus::Unavailable);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for InputMethodState {
    fn event(
        _: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            if capabilities.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(queue_handle, ());
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for InputMethodState {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => state.popup.pointer_entered(&surface, surface_x, surface_y),
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => state.popup.pointer_moved(surface_x, surface_y),
            wl_pointer::Event::Leave { .. } => state.popup.pointer_left(),
            wl_pointer::Event::Button {
                button: LEFT_MOUSE_BUTTON,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => state.popup.primary_button_pressed(),
            _ => {}
        }
    }
}

impl Dispatch<WlSurface, ()> for InputMethodState {
    fn event(
        state: &mut Self,
        _: &WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event {
            state.popup.adopt_buffer_scale(factor, queue_handle);
        }
    }
}

impl Dispatch<WlBuffer, ()> for InputMethodState {
    fn event(
        state: &mut Self,
        buffer: &WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.popup.release_buffer(buffer, queue_handle);
        }
    }
}

impl Dispatch<WlCallback, ()> for InputMethodState {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.popup.frame_presented(queue_handle);
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for InputMethodState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(InputMethodState: ZwpInputMethodManagerV2);
delegate_noop!(InputMethodState: WlCompositor);
delegate_noop!(InputMethodState: ignore WlShm);
delegate_noop!(InputMethodState: WlShmPool);
delegate_noop!(InputMethodState: ignore ZwpInputPopupSurfaceV2);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_and_pin_purposes_are_sensitive() {
        assert!(is_sensitive_content(
            ContentHint::empty(),
            ContentPurpose::Password
        ));
        assert!(is_sensitive_content(
            ContentHint::empty(),
            ContentPurpose::Pin
        ));
        assert!(!is_sensitive_content(
            ContentHint::empty(),
            ContentPurpose::Normal
        ));
    }

    #[test]
    fn hidden_or_sensitive_hints_are_sensitive() {
        assert!(is_sensitive_content(
            ContentHint::HiddenText,
            ContentPurpose::Normal
        ));
        assert!(is_sensitive_content(
            ContentHint::SensitiveData | ContentHint::Spellcheck,
            ContentPurpose::Normal
        ));
        assert!(!is_sensitive_content(
            ContentHint::Spellcheck,
            ContentPurpose::Email
        ));
    }
}
