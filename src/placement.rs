use crate::protocol::PlacementHints;
use log::{debug, warn};
use std::collections::BTreeMap;
use std::fs::File;
use std::os::fd::AsFd;
use std::time::{Duration, Instant};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_pointer, wl_region, wl_registry, wl_seat, wl_shm,
    wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1;
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub overlay_width: i32,
    pub overlay_height: i32,
    pub output_width: i32,
    pub output_height: i32,
}

#[derive(Debug, Clone, Default)]
pub struct PickerPlacement {
    pub geometry: Placement,
    pub output: Option<String>,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            x: 24.0,
            y: 24.0,
            overlay_width: 420,
            overlay_height: 520,
            output_width: 0,
            output_height: 0,
        }
    }
}

impl Placement {
    pub fn from_hints(hints: &PlacementHints) -> Self {
        let overlay_width = hints.overlay_width.unwrap_or(420);
        let overlay_height = hints.overlay_height.unwrap_or(520);
        let output_width = hints.output_width.unwrap_or(0);
        let output_height = hints.output_height.unwrap_or(0);
        let x = hints.pointer_x.unwrap_or_else(|| {
            if output_width > overlay_width {
                ((output_width - overlay_width) / 2) as f64
            } else {
                24.0
            }
        });
        let y = hints.pointer_y.unwrap_or_else(|| {
            if output_height > overlay_height {
                ((output_height - overlay_height) / 2) as f64
            } else {
                24.0
            }
        });
        Self {
            x,
            y,
            overlay_width,
            overlay_height,
            output_width,
            output_height,
        }
    }
}

impl PickerPlacement {
    pub fn from_hints(hints: &PlacementHints) -> Self {
        Self {
            geometry: Placement::from_hints(hints),
            output: hints.output.clone(),
        }
    }
}

pub struct PointerCapture;

impl PointerCapture {
    pub fn capture() -> Result<Placement, Box<dyn std::error::Error>> {
        Self::capture_timeout(Duration::from_secs(30))
    }

    pub fn capture_timeout(timeout: Duration) -> Result<Placement, Box<dyn std::error::Error>> {
        Self::capture_picker_timeout(timeout).map(|placement| placement.geometry)
    }

    pub fn capture_picker_timeout(
        timeout: Duration,
    ) -> Result<PickerPlacement, Box<dyn std::error::Error>> {
        let conn = Connection::connect_to_env()?;
        let (globals, mut queue): (GlobalList, EventQueue<State>) = registry_queue_init(&conn)?;
        let mut state = State::new();
        queue.roundtrip(&mut state)?;
        init_protocols(&globals, &queue, &mut state)?;
        setup_capture_layer(&mut state, &queue)?;

        let deadline = Instant::now() + timeout;
        while !state.coords_received {
            dispatch_once_until(&mut queue, &mut state, deadline)?;
        }
        cleanup_capture_layer(&mut state);
        Ok(PickerPlacement {
            geometry: Placement {
                x: state.received_x,
                y: state.received_y,
                overlay_width: state.overlay_width,
                overlay_height: state.overlay_height,
                output_width: state.monitor_width,
                output_height: state.monitor_height,
            },
            output: state
                .entered_output_id
                .and_then(|id| state.output_names.get(&id).cloned()),
        })
    }
}

fn dispatch_once_until(
    queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    if queue.dispatch_pending(state)? > 0 {
        return Ok(());
    }
    queue.flush()?;
    let Some(guard) = queue.prepare_read() else {
        return Ok(());
    };
    let now = Instant::now();
    if now >= deadline {
        drop(guard);
        return Err("timed out waiting for pointer enter".into());
    }
    let timeout_duration = deadline.saturating_duration_since(now);
    let timeout = rustix::event::Timespec::try_from(timeout_duration)
        .map_err(|_| "pointer capture timeout is too large")?;
    let mut fds = [rustix::event::PollFd::from_borrowed_fd(
        guard.connection_fd(),
        rustix::event::PollFlags::IN
            | rustix::event::PollFlags::ERR
            | rustix::event::PollFlags::HUP,
    )];
    match rustix::event::poll(&mut fds, Some(&timeout)) {
        Ok(0) => {
            drop(guard);
            Err("timed out waiting for pointer enter".into())
        }
        Ok(_) => match pointer_poll_action(fds[0].revents()) {
            PointerPollAction::Read => {
                guard.read()?;
                queue.dispatch_pending(state)?;
                Ok(())
            }
            PointerPollAction::Closed => {
                drop(guard);
                Err("Wayland connection closed during pointer capture".into())
            }
            PointerPollAction::Ignore => {
                drop(guard);
                Ok(())
            }
        },
        Err(rustix::io::Errno::INTR) => Ok(()),
        Err(error) => {
            drop(guard);
            Err(format!("poll pointer capture fd: {error}").into())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointerPollAction {
    Read,
    Closed,
    Ignore,
}

fn pointer_poll_action(revents: rustix::event::PollFlags) -> PointerPollAction {
    if revents.contains(rustix::event::PollFlags::IN) {
        PointerPollAction::Read
    } else if revents.intersects(rustix::event::PollFlags::ERR | rustix::event::PollFlags::HUP) {
        PointerPollAction::Closed
    } else {
        PointerPollAction::Ignore
    }
}

pub struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pointer: Option<wl_pointer::WlPointer>,
    seat: Option<wl_seat::WlSeat>,
    outputs: BTreeMap<u32, wl_output::WlOutput>,
    output_names: BTreeMap<u32, String>,
    entered_output_id: Option<u32>,
    single_pixel_buffer_manager:
        Option<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    shm: Option<wl_shm::WlShm>,
    shm_pool: Option<wl_shm_pool::WlShmPool>,
    shm_file: Option<File>,
    transparent_buffer_size: Option<(i32, i32)>,
    coords_received: bool,
    received_x: f64,
    received_y: f64,
    capture_surface: Option<wl_surface::WlSurface>,
    transparent_buffer: Option<wl_buffer::WlBuffer>,
    capture_layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    capture_viewport: Option<wp_viewport::WpViewport>,
    overlay_width: i32,
    overlay_height: i32,
    monitor_width: i32,
    monitor_height: i32,
}

impl State {
    fn new() -> Self {
        Self {
            compositor: None,
            layer_shell: None,
            pointer: None,
            seat: None,
            outputs: BTreeMap::new(),
            output_names: BTreeMap::new(),
            entered_output_id: None,
            single_pixel_buffer_manager: None,
            viewporter: None,
            shm: None,
            shm_pool: None,
            shm_file: None,
            transparent_buffer_size: None,
            coords_received: false,
            received_x: 24.0,
            received_y: 24.0,
            capture_surface: None,
            transparent_buffer: None,
            capture_layer_surface: None,
            capture_viewport: None,
            overlay_width: 420,
            overlay_height: 520,
            monitor_width: 0,
            monitor_height: 0,
        }
    }
}

fn init_protocols(
    globals: &GlobalList,
    queue: &EventQueue<State>,
    state: &mut State,
) -> Result<(), Box<dyn std::error::Error>> {
    state.compositor =
        Some(globals.bind::<wl_compositor::WlCompositor, _, _>(&queue.handle(), 4..=5, ())?);
    state.layer_shell = Some(globals.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(
        &queue.handle(),
        4..=4,
        (),
    )?);
    state.seat = Some(globals.bind::<wl_seat::WlSeat, _, _>(&queue.handle(), 1..=1, ())?);
    state.viewporter = globals
        .bind::<wp_viewporter::WpViewporter, _, _>(&queue.handle(), 1..=1, ())
        .ok();
    state.single_pixel_buffer_manager = globals
        .bind::<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1, _, _>(
            &queue.handle(),
            1..=1,
            (),
        )
        .ok();
    if state.single_pixel_buffer_manager.is_none() || state.viewporter.is_none() {
        state.shm = globals
            .bind::<wl_shm::WlShm, _, _>(&queue.handle(), 1..=1, ())
            .ok();
    }
    Ok(())
}

fn setup_capture_layer(
    state: &mut State,
    queue: &EventQueue<State>,
) -> Result<(), Box<dyn std::error::Error>> {
    let compositor = state
        .compositor
        .as_ref()
        .ok_or("wl_compositor missing")?
        .clone();
    let capture_surface = compositor.create_surface(&queue.handle(), ());
    state.capture_surface = Some(capture_surface.clone());
    if state.viewporter.is_some() {
        create_transparent_buffer(state, &queue.handle(), 1, 1)?;
    }
    let layer_shell = state.layer_shell.as_ref().ok_or("layer shell missing")?;
    let capture_layer_surface = layer_shell.get_layer_surface(
        &capture_surface,
        None,
        zwlr_layer_shell_v1::Layer::Overlay,
        "d2b-clip-picker-capture".to_string(),
        &queue.handle(),
        (),
    );
    capture_layer_surface.set_exclusive_zone(-1);
    capture_layer_surface.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right
            | zwlr_layer_surface_v1::Anchor::Bottom,
    );
    capture_layer_surface
        .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
    state.capture_layer_surface = Some(capture_layer_surface);
    capture_surface.commit();
    Ok(())
}

fn create_transparent_buffer(
    state: &mut State,
    qhandle: &QueueHandle<State>,
    width: i32,
    height: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    if width <= 0 || height <= 0 {
        return Err("transparent buffer dimensions must be positive".into());
    }
    if state.transparent_buffer_size == Some((width, height)) {
        return Ok(());
    }
    clear_transparent_buffer(state);

    if width == 1
        && height == 1
        && let Some(spbm) = state.single_pixel_buffer_manager.as_ref()
    {
        state.transparent_buffer =
            Some(spbm.create_u32_rgba_buffer(0x00, 0x00, 0x00, 0x00, qhandle, ()));
        state.transparent_buffer_size = Some((width, height));
        return Ok(());
    }

    let shm = state.shm.as_ref().ok_or("wl_shm unavailable")?;
    let stride = width
        .checked_mul(4)
        .ok_or("transparent buffer stride overflow")?;
    let size = stride
        .checked_mul(height)
        .ok_or("transparent buffer size overflow")?;
    let fd = rustix::fs::memfd_create("d2b-clip-picker-shm", rustix::fs::MemfdFlags::CLOEXEC)?;
    let file = File::from(fd);
    file.set_len(size as u64)?;
    let pool = shm.create_pool(file.as_fd(), size, qhandle, ());
    let buffer = pool.create_buffer(
        0,
        width,
        height,
        stride,
        wl_shm::Format::Argb8888,
        qhandle,
        (),
    );
    state.shm_pool = Some(pool);
    state.shm_file = Some(file);
    state.transparent_buffer = Some(buffer);
    state.transparent_buffer_size = Some((width, height));
    Ok(())
}

fn cleanup_capture_layer(state: &mut State) {
    if let Some(viewport) = state.capture_viewport.take() {
        viewport.destroy();
    }
    if let Some(layer_surface) = state.capture_layer_surface.take() {
        layer_surface.destroy();
    }
    if let Some(surface) = state.capture_surface.take() {
        surface.destroy();
    }
    clear_transparent_buffer(state);
}

fn clear_transparent_buffer(state: &mut State) {
    if let Some(buffer) = state.transparent_buffer.take() {
        buffer.destroy();
    }
    if let Some(pool) = state.shm_pool.take() {
        pool.destroy();
    }
    state.shm_file.take();
    state.transparent_buffer_size = None;
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event
            && let WEnum::Value(capabilities) = capabilities
            && capabilities.contains(wl_seat::Capability::Pointer)
        {
            state.pointer = Some(seat.get_pointer(qhandle, ()))
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        if let wl_pointer::Event::Enter {
            surface_x,
            surface_y,
            ..
        } = event
        {
            state.coords_received = true;
            state.received_x = surface_x;
            state.received_y = surface_y;
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.output_names.insert(output.id().protocol_id(), name);
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn event(
        state: &mut Self,
        _surface: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        if let wl_surface::Event::Enter { output } = event {
            state.entered_output_id = Some(output.id().protocol_id());
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut State,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<State>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            layer_surface.ack_configure(serial);
            if width > 0 && height > 0 {
                state.monitor_width = width as i32;
                state.monitor_height = height as i32;
            }
            if state.capture_surface.is_some() {
                if state.viewporter.is_none()
                    && let Err(error) =
                        create_transparent_buffer(state, qhandle, width as i32, height as i32)
                {
                    warn!("failed to create pointer-capture buffer: {error}");
                    return;
                }
                let Some(surface) = &state.capture_surface else {
                    return;
                };
                if let Some(buffer) = &state.transparent_buffer {
                    surface.attach(Some(buffer), 0, 0);
                }
                if state.capture_viewport.is_none()
                    && let Some(viewporter) = &state.viewporter
                {
                    state.capture_viewport = Some(viewporter.get_viewport(surface, qhandle, ()))
                }
                if let Some(viewport) = &state.capture_viewport {
                    viewport.set_destination(width as i32, height as i32);
                }
                surface.damage(0, 0, width as i32, height as i32);
                surface.commit();
            }
        }
    }
}

delegate_noop!(State: wl_compositor::WlCompositor);
delegate_noop!(State: wl_region::WlRegion);
delegate_noop!(State: zwlr_layer_shell_v1::ZwlrLayerShellV1);
delegate_noop!(State: wp_viewporter::WpViewporter);
delegate_noop!(State: wp_viewport::WpViewport);
delegate_noop!(State: wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_shm::WlShm);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut State,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        qh: &QueueHandle<State>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "wl_output" {
                let output =
                    registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                state.outputs.insert(output.id().protocol_id(), output);
            }
        } else {
            debug!("registry event ignored by pointer capture");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_poll_action_prioritizes_read_before_hup() {
        let action =
            pointer_poll_action(rustix::event::PollFlags::IN | rustix::event::PollFlags::HUP);

        assert_eq!(action, PointerPollAction::Read);
    }
}
