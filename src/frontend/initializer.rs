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
use crate::frontend::ipc_client::FrontendClient;
use crate::frontend::{frontend_state::State, gtk_overlay};
use log::{debug, error, warn};
use memmap2::{MmapMut, MmapOptions};
use std::fs::OpenOptions;
use std::os::fd::BorrowedFd;
use std::os::unix::io::AsRawFd;

fn run_main_event_loop(
    state: &mut State,
    queue: &mut EventQueue<State>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut gtk_window_created = false;

    loop {
        // Process Wayland events
        queue.blocking_dispatch(state)?;

        // Create GTK overlay window when coordinates are received
        if state.coords_received && !gtk_window_created {
            let x = state.received_x;
            let y = state.received_y;
            debug!("Capture layer ready; creating GTK overlay window at ({x}, {y})");

            // Create the GTK window using the unified client backend communication
            if let Err(e) =
                gtk_overlay::init_clipboard_overlay(x, y, state.clipboard_history.clone())
            {
                error!("Error creating GTK overlay: {e:?}");
            }

            gtk_window_created = true;
        }

        // Handle close requests
        if gtk_window_created && (gtk_overlay::is_close_requested() || state.capture_layer_clicked)
        {
            gtk_overlay::reset_close_flags();
            cleanup_capture_layer(state);
            break;
        }

        // Process GTK events if window has been created
        if gtk_window_created {
            gtk4::glib::MainContext::default().iteration(false);
        }
    }

    Ok(())
}

// Frontend always uses its own Wayland connection (may change in future to support shared connection/hide feature)
pub async fn run_frontend() -> Result<(), Box<dyn std::error::Error>> {
    let mut state = State::new();
    // Prefetch clipboard history for instant GTK overlay population
    if let Ok(mut client) = FrontendClient::new() {
        match client.get_history() {
            Ok(items) => {
                state.clipboard_history = items;
                debug!(
                    "Prefetched {} clipboard history items",
                    state.clipboard_history.len()
                );
            }
            Err(e) => warn!("Failed to prefetch clipboard history: {e}"),
        }
    } else {
        warn!("Failed to connect to backend for history prefetch");
    }

    // Initialize Wayland for layer shell capture
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue): (GlobalList, EventQueue<State>) =
        registry_queue_init::<State>(&conn)?;

    queue.roundtrip(&mut state)?;

    // Initialize Wayland protocols
    init_wayland_protocols(&globals, &queue, &mut state)?;

    // Create capture surfaces for mouse coordinate detection
    setup_capture_layer(&mut state, &queue);

    // Main event loop (reuse existing implementation)
    run_main_event_loop(&mut state, &mut queue)
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

    // Fallback: wl_shm 1x1 ARGB8888 buffer backed by a temporary file + memmap
    let shm = state
        .shm
        .as_ref()
        .ok_or_else(|| std::io::Error::other("wl_shm not available for fallback"))?;

    // 1x1 pixel ARGB8888 (4 bytes)
    let size: i32 = 4;

    // Create a unique temp file (unlinked after creation) to back the SHM pool
    let mut path = std::env::temp_dir();
    let unique = format!(
        "cursor-clip-shm-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    path.push(unique);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.set_len(size as u64)?;

    // Map and zero the file to ensure transparent pixel content
    let mut mmap: MmapMut = unsafe { MmapOptions::new().len(size as usize).map_mut(&file)? };
    mmap.fill(0);
    mmap.flush()?;

    // We can unlink the file path now; the file remains alive via the handle
    let _ = std::fs::remove_file(&path);

    // Create wl_shm_pool using a borrowed fd from the File
    let borrow_fd: BorrowedFd<'_> = unsafe { BorrowedFd::borrow_raw(file.as_raw_fd()) };
    let pool = shm.create_pool(borrow_fd, size, &queue.handle(), ());
    let buffer = pool.create_buffer(
        0, // offset
        1,
        1, // width, height
        4, // stride (bytes per row)
        wl_shm::Format::Argb8888,
        &queue.handle(),
        (),
    );
    // Keep pool and file alive for the lifetime of the buffer
    state.shm_file = Some(file);
    state.shm_pool = Some(pool);
    state.transparent_buffer = Some(buffer);
    Ok(())
}
