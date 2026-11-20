use wayland_client::{
    Connection, EventQueue,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_compositor, wl_seat, wl_shm},
};
use wayland_protocols::wp::{
    single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1,
    viewporter::client::wp_viewporter,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::frontend::dispatch::layer_shell::cleanup_capture_layer;
use crate::frontend::toggle::ToggleServer;
use crate::frontend::{frontend_state::State, gtk_overlay, hyprland};
use crate::shared::config::{PopupPosition, UserConfig, config_path};
use log::{debug, error, info, warn};
use rustix::fs::{MemfdFlags, memfd_create};
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, AsRawFd};

use super::OverlayLocation;

fn run_main_event_loop(
    state: &mut State,
    queue: &mut EventQueue<State>,
    toggle_server: &ToggleServer,
    config: UserConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        // Wait for either compositor activity or another invocation. Monitoring
        // both descriptors prevents additional capture layers from queueing.
        if !dispatch_wayland_or_toggle(state, queue, toggle_server)? {
            cleanup_capture_layer(state);
            break;
        }

        if state.coords_received {
            let x = state.received_x;
            let y = state.received_y;
            debug!("Capture layer ready; creating GTK overlay window at ({x}, {y})");

            // The overlay window covers the output itself and handles outside
            // clicks, so the capture layer is no longer needed.
            cleanup_capture_layer(state);
            queue.flush()?;

            show_overlay(
                OverlayLocation {
                    cursor: (x, y),
                    monitor_width: state.monitor_width,
                    monitor_height: state.monitor_height,
                    monitor: None,
                },
                toggle_server,
                false,
                config,
            )?;
            break;
        }
    }

    Ok(())
}

fn show_overlay(
    location: OverlayLocation,
    toggle_server: &ToggleServer,
    resident: bool,
    config: UserConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Err(e) = gtk_overlay::init_clipboard_overlay(
        location,
        toggle_server.try_clone_listener()?,
        resident,
        config,
    ) {
        error!("Error creating GTK overlay: {e:?}");
    }
    gtk_overlay::reset_close_flags();
    Ok(())
}

fn dispatch_wayland_or_toggle(
    state: &mut State,
    queue: &mut EventQueue<State>,
    toggle_server: &ToggleServer,
) -> Result<bool, Box<dyn std::error::Error>> {
    if toggle_server.take_toggle_request()? {
        return Ok(false);
    }

    if queue.dispatch_pending(state)? > 0 {
        return Ok(true);
    }
    queue.flush()?;

    loop {
        if toggle_server.take_toggle_request()? {
            return Ok(false);
        }

        let Some(read_guard) = queue.prepare_read() else {
            if queue.dispatch_pending(state)? > 0 {
                return Ok(true);
            }
            continue;
        };

        let mut poll_fds = [
            libc::pollfd {
                fd: read_guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: toggle_server.raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];

        // SAFETY: poll_fds points to two initialized pollfd values and remains
        // alive for the duration of the call.
        let result = unsafe { libc::poll(poll_fds.as_mut_ptr(), poll_fds.len() as _, -1) };
        if result < 0 {
            let error = io::Error::last_os_error();
            drop(read_guard);
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }

        if poll_fds[1].revents != 0 {
            drop(read_guard);
            return Ok(!toggle_server.take_toggle_request()?);
        }

        if poll_fds[0].revents != 0 {
            read_guard.read()?;
            queue.dispatch_pending(state)?;
            return Ok(true);
        }
    }
}

// Frontend always uses its own Wayland connection (may change in future to support shared connection/hide feature)
pub fn run_frontend() -> Result<(), Box<dyn std::error::Error>> {
    let Some(toggle_server) = ToggleServer::acquire()? else {
        info!("Requested a toggle of the active overlay");
        return Ok(());
    };

    run_overlay(toggle_server)
}

#[tokio::main]
async fn run_overlay(toggle_server: ToggleServer) -> Result<(), Box<dyn std::error::Error>> {
    let config = gtk_overlay::overlay_config(&config_path()).unwrap_or_default();
    if let Some(location) = hyprland::overlay_location(config.position) {
        // Hyprland can locate the overlay on every reopen without restarting GTK.
        return show_overlay(location, &toggle_server, true, config);
    }
    if config.position != PopupPosition::Cursor {
        // Layer Shell lets the compositor choose the active output.
        return show_overlay(OverlayLocation::default(), &toggle_server, false, config);
    }

    // Initialize Wayland for layer shell capture
    let mut state = State::default();
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue): (GlobalList, EventQueue<State>) =
        registry_queue_init::<State>(&conn)?;

    queue.roundtrip(&mut state)?;

    // Initialize Wayland protocols
    init_wayland_protocols(&globals, &queue, &mut state)?;

    // Create capture surfaces for mouse coordinate detection
    setup_capture_layer(&mut state, &queue);

    // Main event loop (reuse existing implementation)
    run_main_event_loop(&mut state, &mut queue, &toggle_server, config)
}

fn init_wayland_protocols(
    globals: &GlobalList,
    queue: &EventQueue<State>,
    state: &mut State,
) -> Result<(), Box<dyn std::error::Error>> {
    // Bind wl_compositor
    if let Ok(compositor) =
        globals.bind::<wl_compositor::WlCompositor, _, _>(&queue.handle(), 4..=5, ())
    {
        state.compositor = Some(compositor);
    } else {
        let msg = "Critical Wayland global object (interface) 'wl_compositor' is not available. \
        Your compositor did not advertise wl_compositor (v4-5), so we cannot create the surfaces required for the overlay. \
        Frontend cannot start, exiting.";
        error!("{msg}");
        std::process::exit(1);
    }

    // Bind zwlr_layer_shell_v1
    if let Ok(layer_shell) =
        globals.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(&queue.handle(), 4..=4, ())
    {
        state.layer_shell = Some(layer_shell);
    } else {
        let msg = "Critical Wayland global object (interface) 'zwlr_layer_shell_v1' is not available. \
        Your current compositor likely does not support the wlr-layer-shell protocol (probably running GNOME). \
        Clipboard monitoring cannot function without it, exiting.";
        error!("{msg}");
        std::process::exit(1);
    }

    // Bind wl_seat
    if let Ok(seat) = globals.bind::<wl_seat::WlSeat, _, _>(&queue.handle(), 1..=1, ()) {
        state.seat = Some(seat);
    } else {
        let msg = "Critical Wayland interface 'wl_seat' is not available. \
        An input seat is required to receive pointer events for capture surface interactions. \
        Frontend cannot start, exiting.";
        error!("{msg}");
        std::process::exit(1);
    }

    // Bind wp_viewporter
    if let Ok(viewporter) =
        globals.bind::<wp_viewporter::WpViewporter, _, _>(&queue.handle(), 1..=1, ())
    {
        state.viewporter = Some(viewporter);
    } else {
        debug!("wp_viewporter not available");
    }

    // Bind wp_single_pixel_buffer_manager_v1 (preferred path)
    if let Ok(single_pixel_buffer_manager) =
        globals.bind::<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1, _, _>(
            &queue.handle(),
            1..=1,
            (),
        )
    {
        // No Fallback needed; we have SPBM
        state.single_pixel_buffer_manager = Some(single_pixel_buffer_manager);
    } else {
        // Fallback: bind wl_shm
        warn!("wp_single_pixel_buffer_manager_v1 not available; attempting wl_shm fallback");
        if let Ok(shm) = globals.bind::<wl_shm::WlShm, _, _>(&queue.handle(), 1..=1, ()) {
            state.shm = Some(shm);
        } else {
            let msg = "Neither wp_single_pixel_buffer_manager_v1 nor wl_shm are available; cannot create buffers. Exiting.";
            error!("{msg}");
            std::process::exit(1);
        }
    }

    Ok(())
}

fn setup_capture_layer(state: &mut State, queue: &EventQueue<State>) {
    // Limit the borrow of state by cloning the compositor proxy
    {
        let compositor = state
            .compositor
            .as_ref()
            .expect("Compositor not initialized")
            .clone();

        let capture_surface = compositor.create_surface(&queue.handle(), ());
        let update_surface = compositor.create_surface(&queue.handle(), ());

        state.capture_surface = Some(capture_surface.clone());
        state.update_surface = Some(update_surface);
    }

    // Create the transparent buffer (SPBM or SHM fallback)
    if let Err(e) = create_transparent_buffer(state, queue) {
        error!("Failed to create transparent buffer: {e}");
        std::process::exit(1);
    }

    let layer_shell = state
        .layer_shell
        .as_ref()
        .expect("Layer Shell not initialized");

    let capture_surface_ref = state
        .capture_surface
        .as_ref()
        .expect("capture_surface not initialized");

    let capture_layer_surface = layer_shell.get_layer_surface(
        capture_surface_ref,
        None,
        zwlr_layer_shell_v1::Layer::Overlay,
        "cursor-clip-capture".to_string(),
        &queue.handle(),
        (),
    );

    // Configure the capture layer surface
    capture_layer_surface.set_exclusive_zone(-1);
    capture_layer_surface.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right
            | zwlr_layer_surface_v1::Anchor::Bottom,
    );

    state.capture_layer_surface = Some(capture_layer_surface);
    capture_surface_ref.commit();
}

// Helper to create a 1x1 fully transparent buffer either via SPBM or SHM fallback
fn create_transparent_buffer(
    state: &mut State,
    queue: &EventQueue<State>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Prefer wp_single_pixel_buffer_manager if available
    if let Some(spbm) = state.single_pixel_buffer_manager.as_ref() {
        let transparent_buffer =
            spbm.create_u32_rgba_buffer(0x00, 0x00, 0x00, 0x00, &queue.handle(), ());
        state.transparent_buffer = Some(transparent_buffer);
        return Ok(());
    }

    let shm = state
        .shm
        .as_ref()
        .ok_or_else(|| std::io::Error::other("wl_shm not available for fallback"))?;

    // Extending a new memfd yields four zero bytes, one transparent ARGB8888 pixel.
    let file = File::from(memfd_create(c"cursor-clip-pixel", MemfdFlags::CLOEXEC)?);
    file.set_len(4)?;
    let qh = queue.handle();
    let pool = shm.create_pool(file.as_fd(), 4, &qh, ());
    let buffer = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, &qh, ());
    // wl_buffer retains the backing storage after the pool is destroyed.
    pool.destroy();
    state.transparent_buffer = Some(buffer);
    Ok(())
}
