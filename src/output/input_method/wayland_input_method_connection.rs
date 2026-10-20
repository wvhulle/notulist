use std::error::Error;
use std::sync::{Arc, OnceLock};

use calloop::{channel, EventLoop};
use calloop_wayland_source::WaylandSource;
use tokio::sync::watch;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry::WlRegistry, wl_seat::WlSeat};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::{
    self, ZwpInputMethodV2,
};

use super::preedit_commit_tracker::InputMethodBatch;
use super::TextFieldFocus;

type LoopError = Box<dyn Error + Send + Sync>;

pub struct FieldBatch {
    pub field: u32,
    pub batch: InputMethodBatch,
}

pub struct InputMethodConnection {
    batches: channel::Sender<FieldBatch>,
    focus: watch::Receiver<TextFieldFocus>,
}

static SHARED_CONNECTION: OnceLock<InputMethodConnection> = OnceLock::new();

impl InputMethodConnection {
    pub fn shared() -> &'static InputMethodConnection {
        SHARED_CONNECTION.get_or_init(Self::spawn)
    }

    fn spawn() -> Self {
        let (batches, batch_source) = channel::channel();
        let (focus_sender, focus) = watch::channel(TextFieldFocus::Connecting);
        let focus_sender = Arc::new(focus_sender);
        let thread_focus_sender = Arc::clone(&focus_sender);
        let spawned = std::thread::Builder::new()
            .name("voxtype-input-method".to_owned())
            .spawn(move || {
                if let Err(error) = run_input_method_loop(batch_source, &thread_focus_sender) {
                    tracing::warn!("Input method connection unavailable: {error}");
                    publish_focus(&thread_focus_sender, TextFieldFocus::Unavailable);
                }
            });
        if let Err(error) = spawned {
            tracing::warn!("Could not start the input method thread: {error}");
            publish_focus(&focus_sender, TextFieldFocus::Unavailable);
        }
        Self { batches, focus }
    }

    pub fn focused_field(&self) -> Option<u32> {
        self.focus.borrow().focused_field()
    }

    pub fn focus_changes(&self) -> watch::Receiver<TextFieldFocus> {
        self.focus.clone()
    }

    pub fn send(&self, field: u32, batch: InputMethodBatch) -> bool {
        self.focused_field() == Some(field)
            && self.batches.send(FieldBatch { field, batch }).is_ok()
    }
}

struct InputMethodState {
    input_method: ZwpInputMethodV2,
    done_events: u32,
    pending_focused: bool,
    activated_since_done: bool,
    activated_fields: u32,
    focus: TextFieldFocus,
    focus_sender: Arc<watch::Sender<TextFieldFocus>>,
}

impl InputMethodState {
    fn apply(&self, FieldBatch { field, batch }: FieldBatch) {
        if self.focus.focused_field() != Some(field) {
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

    fn settle_focus(&mut self, focus: TextFieldFocus) {
        self.focus = focus;
        publish_focus(&self.focus_sender, focus);
    }
}

fn publish_focus(sender: &watch::Sender<TextFieldFocus>, focus: TextFieldFocus) {
    sender.send_if_modified(|current| {
        let changed = *current != focus;
        *current = focus;
        changed
    });
}

fn run_input_method_loop(
    batch_source: channel::Channel<FieldBatch>,
    focus_sender: &Arc<watch::Sender<TextFieldFocus>>,
) -> Result<(), LoopError> {
    let connection = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init::<InputMethodState>(&connection)?;
    let queue_handle = event_queue.handle();
    let seat: WlSeat = globals.bind(&queue_handle, 1..=1, ())?;
    let manager: ZwpInputMethodManagerV2 = globals.bind(&queue_handle, 1..=1, ())?;
    let input_method = manager.get_input_method(&seat, &queue_handle, ());

    let mut event_loop: EventLoop<InputMethodState> = EventLoop::try_new()?;
    WaylandSource::new(connection, event_queue)
        .insert(event_loop.handle())
        .map_err(|insert_error| insert_error.error)?;
    event_loop
        .handle()
        .insert_source(batch_source, |event, _, state| {
            if let channel::Event::Msg(field_batch) = event {
                state.apply(field_batch);
            }
        })
        .map_err(|insert_error| insert_error.error)?;

    let mut state = InputMethodState {
        input_method,
        done_events: 0,
        pending_focused: false,
        activated_since_done: false,
        activated_fields: 0,
        focus: TextFieldFocus::Unfocused,
        focus_sender: Arc::clone(focus_sender),
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
                state.pending_focused = true;
                state.activated_since_done = true;
            }
            zwp_input_method_v2::Event::Deactivate => state.pending_focused = false,
            zwp_input_method_v2::Event::Done => {
                state.done_events = state.done_events.wrapping_add(1);
                if std::mem::take(&mut state.activated_since_done) {
                    state.activated_fields = state.activated_fields.wrapping_add(1);
                }
                let focus = if state.pending_focused {
                    TextFieldFocus::Focused {
                        field: state.activated_fields,
                    }
                } else {
                    TextFieldFocus::Unfocused
                };
                state.settle_focus(focus);
            }
            zwp_input_method_v2::Event::Unavailable => {
                tracing::warn!("Another input method already serves this seat");
                state.settle_focus(TextFieldFocus::Unavailable);
            }
            _ => {}
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

delegate_noop!(InputMethodState: ignore WlSeat);
delegate_noop!(InputMethodState: ZwpInputMethodManagerV2);
