use std::io::Write;
use std::os::fd::AsFd;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::{
    self, ZwpInputMethodV2,
};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_popup_surface_v2::{
    self, ZwpInputPopupSurfaceV2,
};

const SQUARE_PIXELS: i32 = 48;
const SQUARE_ARGB: u32 = 0xFF3B82F6;

struct ProbePopup {
    surface: WlSurface,
    popup: ZwpInputPopupSurfaceV2,
}

struct PointerProbe {
    compositor: WlCompositor,
    buffer: WlBuffer,
    input_method: ZwpInputMethodV2,
    popup: Option<ProbePopup>,
    pointer: Option<WlPointer>,
    pointer_over_popup: bool,
    pending_focused: bool,
}

impl PointerProbe {
    fn show_popup(&mut self, queue_handle: &QueueHandle<Self>) {
        let surface = self.compositor.create_surface(queue_handle, ());
        let popup = self
            .input_method
            .get_input_popup_surface(&surface, queue_handle, ());
        surface.attach(Some(&self.buffer), 0, 0);
        surface.damage_buffer(0, 0, SQUARE_PIXELS, SQUARE_PIXELS);
        surface.commit();
        self.popup = Some(ProbePopup { surface, popup });
        println!("text field focused: showing a blue square at the cursor; click it");
    }

    fn hide_popup(&mut self) {
        if let Some(ProbePopup { surface, popup }) = self.popup.take() {
            popup.destroy();
            surface.destroy();
            println!("text field unfocused: popup removed");
        }
    }

    fn is_popup_surface(&self, surface: &WlSurface) -> bool {
        self.popup
            .as_ref()
            .is_some_and(|probe_popup| &probe_popup.surface == surface)
    }
}

fn solid_square_buffer(
    shm: &WlShm,
    queue_handle: &QueueHandle<PointerProbe>,
) -> std::io::Result<WlBuffer> {
    let stride = SQUARE_PIXELS * 4;
    let size = stride * SQUARE_PIXELS;
    let mut file = tempfile::tempfile()?;
    let pixels: Vec<u8> = std::iter::repeat_n(
        SQUARE_ARGB.to_ne_bytes(),
        (SQUARE_PIXELS * SQUARE_PIXELS) as usize,
    )
    .flatten()
    .collect();
    file.write_all(&pixels)?;
    let pool: WlShmPool = shm.create_pool(file.as_fd(), size, queue_handle, ());
    let buffer = pool.create_buffer(
        0,
        SQUARE_PIXELS,
        SQUARE_PIXELS,
        stride,
        wl_shm::Format::Argb8888,
        queue_handle,
        (),
    );
    pool.destroy();
    Ok(buffer)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env().map_err(|error| {
        format!("cannot reach the Wayland compositor ({error}); run this from inside your desktop session")
    })?;
    let (globals, mut event_queue) = registry_queue_init::<PointerProbe>(&connection)?;
    let queue_handle = event_queue.handle();
    let compositor: WlCompositor = globals.bind(&queue_handle, 1..=4, ())?;
    let shm: WlShm = globals.bind(&queue_handle, 1..=1, ())?;
    let seat: WlSeat = globals.bind(&queue_handle, 1..=5, ())?;
    let manager: ZwpInputMethodManagerV2 = globals.bind(&queue_handle, 1..=1, ())?;
    let input_method = manager.get_input_method(&seat, &queue_handle, ());
    let buffer = solid_square_buffer(&shm, &queue_handle)?;

    let mut probe = PointerProbe {
        compositor,
        buffer,
        input_method,
        popup: None,
        pointer: None,
        pointer_over_popup: false,
        pending_focused: false,
    };
    println!("Probe running. Focus a text field, then click the blue square. Ctrl+C to quit.");
    loop {
        event_queue.blocking_dispatch(&mut probe)?;
    }
}

impl Dispatch<ZwpInputMethodV2, ()> for PointerProbe {
    fn event(
        probe: &mut Self,
        _: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::Activate => probe.pending_focused = true,
            zwp_input_method_v2::Event::Deactivate => probe.pending_focused = false,
            zwp_input_method_v2::Event::Done => {
                match (probe.pending_focused, probe.popup.is_some()) {
                    (true, false) => probe.show_popup(queue_handle),
                    (false, true) => probe.hide_popup(),
                    _ => {}
                }
            }
            zwp_input_method_v2::Event::Unavailable => {
                eprintln!("Another input method holds the seat; stop it first (systemctl --user stop voxtype).");
                std::process::exit(1);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpInputPopupSurfaceV2, ()> for PointerProbe {
    fn event(
        _: &mut Self,
        _: &ZwpInputPopupSurfaceV2,
        event: zwp_input_popup_surface_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_input_popup_surface_v2::Event::TextInputRectangle {
            x,
            y,
            width,
            height,
        } = event
        {
            println!("text cursor rectangle relative to the popup: x={x} y={y} {width}x{height}");
        }
    }
}

impl Dispatch<WlSeat, ()> for PointerProbe {
    fn event(
        probe: &mut Self,
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
            if capabilities.contains(wl_seat::Capability::Pointer) && probe.pointer.is_none() {
                probe.pointer = Some(seat.get_pointer(queue_handle, ()));
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for PointerProbe {
    fn event(
        probe: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface, .. } if probe.is_popup_surface(&surface) => {
                probe.pointer_over_popup = true;
                println!("POINTER ENTERED the popup");
            }
            wl_pointer::Event::Leave { surface, .. } if probe.is_popup_surface(&surface) => {
                probe.pointer_over_popup = false;
                println!("pointer left the popup");
            }
            wl_pointer::Event::Button { button, state, .. } if probe.pointer_over_popup => {
                println!("BUTTON {button:#x} {state:?} on the popup; watch whether the field stays focused");
            }
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for PointerProbe {
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

delegate_noop!(PointerProbe: WlCompositor);
delegate_noop!(PointerProbe: ignore WlSurface);
delegate_noop!(PointerProbe: ignore WlShm);
delegate_noop!(PointerProbe: WlShmPool);
delegate_noop!(PointerProbe: ignore WlBuffer);
delegate_noop!(PointerProbe: ZwpInputMethodManagerV2);
