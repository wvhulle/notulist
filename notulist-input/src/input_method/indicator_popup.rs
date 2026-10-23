use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Proxy, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2;

use super::dictation_indicator_painter::{resolve_font, PillPainter, SurfacePoint};
use super::indicator_phase::{IndicatorFrame, IndicatorPhase};
use super::shm::{BufferDimensions, ShmDoubleBuffer};
use super::wayland_input_method_connection::InputMethodState;
use super::TextFieldFocus;
use crate::ime_config::PopupConfig;

const SPINNER_TURNS_PER_SECOND: f32 = 1.2;

pub struct IndicatorPopup {
    compositor: WlCompositor,
    shm: WlShm,
    enabled: bool,
    animations_enabled: bool,
    painter: PillPainter,
    frame: IndicatorFrame,
    focused_field: Option<u32>,
    shown: Option<ShownPopup>,
    animation_started: Instant,
    pointer_inside: bool,
    pointer_position: SurfacePoint,
    mic_clicks: UnboundedSender<()>,
}

struct ShownPopup {
    surface: WlSurface,
    popup: ZwpInputPopupSurfaceV2,
    buffers: Option<ShmDoubleBuffer>,
    retired_buffers: Vec<ShmDoubleBuffer>,
    buffer_scale: i32,
    frame_callback_pending: bool,
    redraw_wanted: bool,
}

impl ShownPopup {
    fn retire_current_buffers(&mut self) {
        let Some(buffers) = self.buffers.take() else {
            return;
        };
        if !buffers.all_slots_released() {
            self.retired_buffers.push(buffers);
        }
    }

    fn mark_buffer_released(&mut self, buffer: &WlBuffer) {
        if let Some(buffers) = self.buffers.as_mut() {
            buffers.mark_released(buffer);
        }
        self.retired_buffers
            .iter_mut()
            .for_each(|retired| retired.mark_released(buffer));
        self.retired_buffers
            .retain(|retired| !retired.all_slots_released());
    }

    fn buffers_sized_for(
        &mut self,
        dimensions: BufferDimensions,
        shm: &WlShm,
        queue_handle: &QueueHandle<InputMethodState>,
    ) -> Option<&mut ShmDoubleBuffer> {
        if self
            .buffers
            .as_ref()
            .is_some_and(|buffers| buffers.dimensions() != dimensions)
        {
            self.retire_current_buffers();
        }
        if self.buffers.is_none() {
            match ShmDoubleBuffer::new(shm, queue_handle, dimensions) {
                Ok(buffers) => self.buffers = Some(buffers),
                Err(error) => {
                    tracing::warn!("Could not allocate the indicator buffer: {error}");
                    return None;
                }
            }
        }
        self.buffers.as_mut()
    }
}

impl IndicatorPopup {
    pub fn new(
        compositor: WlCompositor,
        shm: WlShm,
        popup: &PopupConfig,
        mic_clicks: UnboundedSender<()>,
    ) -> Self {
        Self {
            compositor,
            shm,
            enabled: popup.show_popup,
            animations_enabled: popup.animations_enabled,
            painter: PillPainter::new(popup, resolve_font(popup.font_file.as_deref())),
            frame: IndicatorFrame {
                phase: IndicatorPhase::Hidden,
                level_bar_heights: Vec::new(),
            },
            focused_field: None,
            shown: None,
            animation_started: Instant::now(),
            pointer_inside: false,
            pointer_position: SurfacePoint { x: 0.0, y: 0.0 },
            mic_clicks,
        }
    }

    pub fn follow_focus(
        &mut self,
        focus: TextFieldFocus,
        input_method: &ZwpInputMethodV2,
        queue_handle: &QueueHandle<InputMethodState>,
    ) {
        let field = focus.focused_field();
        if field != self.focused_field {
            self.hide();
            self.focused_field = field;
        }
        self.sync_visibility(input_method, queue_handle);
    }

    pub fn show_indicator_frame(
        &mut self,
        frame: IndicatorFrame,
        input_method: &ZwpInputMethodV2,
        queue_handle: &QueueHandle<InputMethodState>,
    ) {
        self.frame = frame;
        self.sync_visibility(input_method, queue_handle);
    }

    fn sync_visibility(
        &mut self,
        input_method: &ZwpInputMethodV2,
        queue_handle: &QueueHandle<InputMethodState>,
    ) {
        let wants_popup = self.enabled
            && self.focused_field.is_some()
            && self.frame.phase != IndicatorPhase::Hidden;
        match (wants_popup, self.shown.is_some()) {
            (true, false) => self.show(input_method, queue_handle),
            (false, true) => self.hide(),
            (true, true) => self.request_redraw(queue_handle),
            (false, false) => {}
        }
    }

    fn show(
        &mut self,
        input_method: &ZwpInputMethodV2,
        queue_handle: &QueueHandle<InputMethodState>,
    ) {
        let surface = self.compositor.create_surface(queue_handle, ());
        let popup = input_method.get_input_popup_surface(&surface, queue_handle, ());
        self.shown = Some(ShownPopup {
            surface,
            popup,
            buffers: None,
            retired_buffers: Vec::new(),
            buffer_scale: 1,
            frame_callback_pending: false,
            redraw_wanted: true,
        });
        self.pointer_inside = false;
        self.redraw_now(queue_handle);
    }

    fn hide(&mut self) {
        if let Some(shown) = self.shown.take() {
            shown.popup.destroy();
            shown.surface.destroy();
        }
        self.pointer_inside = false;
    }

    pub fn adopt_buffer_scale(&mut self, scale: i32, queue_handle: &QueueHandle<InputMethodState>) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        if shown.buffer_scale != scale.max(1) {
            shown.buffer_scale = scale.max(1);
            shown.retire_current_buffers();
            self.request_redraw(queue_handle);
        }
    }

    pub fn release_buffer(
        &mut self,
        buffer: &WlBuffer,
        queue_handle: &QueueHandle<InputMethodState>,
    ) {
        let wanted = self.shown.as_mut().is_some_and(|shown| {
            shown.mark_buffer_released(buffer);
            shown.redraw_wanted && !shown.frame_callback_pending
        });
        if wanted {
            self.redraw_now(queue_handle);
        }
    }

    pub fn frame_presented(&mut self, queue_handle: &QueueHandle<InputMethodState>) {
        let animated = self.frame.phase.is_animated() && self.animations_enabled;
        let wanted = self.shown.as_mut().is_some_and(|shown| {
            shown.frame_callback_pending = false;
            shown.redraw_wanted || animated
        });
        if wanted {
            self.redraw_now(queue_handle);
        }
    }

    fn request_redraw(&mut self, queue_handle: &QueueHandle<InputMethodState>) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        shown.redraw_wanted = true;
        if !shown.frame_callback_pending {
            self.redraw_now(queue_handle);
        }
    }

    fn redraw_now(&mut self, queue_handle: &QueueHandle<InputMethodState>) {
        let spinner_turns =
            self.animation_started.elapsed().as_secs_f32() * SPINNER_TURNS_PER_SECOND;
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        let scale = shown.buffer_scale;
        let Some(pill) =
            self.painter
                .paint(&self.frame, spinner_turns.fract(), scale.max(1) as u32)
        else {
            return;
        };
        let dimensions = BufferDimensions {
            width: pill.size.width as i32,
            height: pill.size.height as i32,
        };
        let Some(buffers) = shown.buffers_sized_for(dimensions, &self.shm, queue_handle) else {
            return;
        };
        let Some(slot) = buffers.acquire() else {
            shown.redraw_wanted = true;
            return;
        };
        if let Err(error) = buffers.write_rgba(slot, &pill.rgba) {
            buffers.return_unused_slot(slot);
            tracing::warn!("Could not write the indicator buffer: {error}");
            return;
        }
        let attached = buffers.buffer(slot).clone();
        shown.surface.set_buffer_scale(scale);
        shown.surface.attach(Some(&attached), 0, 0);
        shown
            .surface
            .damage_buffer(0, 0, dimensions.width, dimensions.height);
        shown.surface.frame(queue_handle, ());
        shown.surface.commit();
        shown.frame_callback_pending = true;
        shown.redraw_wanted = false;
    }

    pub fn pointer_entered(&mut self, surface: &WlSurface, x: f64, y: f64) {
        self.pointer_inside = self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.surface.id() == surface.id());
        self.pointer_position = SurfacePoint { x, y };
    }

    pub fn pointer_moved(&mut self, x: f64, y: f64) {
        self.pointer_position = SurfacePoint { x, y };
    }

    pub fn pointer_left(&mut self) {
        self.pointer_inside = false;
    }

    pub fn primary_button_pressed(&mut self) {
        if self.pointer_inside
            && self.frame.phase.accepts_mic_click()
            && self
                .painter
                .mic_button_contains(self.frame.phase, self.pointer_position)
        {
            let _ = self.mic_clicks.send(());
        }
    }
}
