use wayland_client::{
    Connection, EventQueue,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_compositor, wl_seat},
};
use wayland_protocols_wlr::{
    layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
    virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1,
};
use wayland_protocols::{
    wp::{
        single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1,
        viewporter::client::wp_viewporter,
    },
};

use crate::frontend::{frontend_state::State, gtk_overlay};
use crate::frontend::frontend_client::FrontendClient;

async fn run_main_event_loop(
    state: &mut State, 
    queue: &mut EventQueue<State>
) -> Result<(), Box<dyn std::error::Error>> {
    let mut gtk_window_created = false;
    
    loop {
        // Process Wayland events
        queue.blocking_dispatch(state)?;

        // Create GTK overlay window when coordinates are received
        if state.coords_received && !gtk_window_created {
            let x = state.received_x;
            let y = state.received_y;
            println!("Capture layer ready! Creating GTK overlay window at ({}, {})...", x, y);

            // Create the GTK window using the unified client backend communication
            if let Err(e) = gtk_overlay::create_clipboard_overlay(x, y, state.clipboard_history.clone()) {
                eprintln!("Error creating GTK overlay: {:?}", e);
            }
            
            gtk_window_created = true;
        }
        
        // Handle close requests
        if gtk_window_created && (gtk_overlay::is_close_requested() || state.capture_layer_clicked) {
            println!("Close requested - closing both capture layer and GTK window");
            
            // Close GTK overlay window
            gtk_overlay::reset_close_flags();
            
            // Clean up capture layer surface
            if let Some(capture_layer_surface) = &state.capture_layer_surface {
                capture_layer_surface.destroy();
                println!("Capture layer surface destroyed");
            }
            state.capture_layer_surface = None;
            state.capture_layer_clicked = false;
            
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
                println!("Prefetched {} clipboard history items", state.clipboard_history.len());
            }
            Err(e) => eprintln!("Failed to prefetch clipboard history: {e}"),
        }
    } else {
        eprintln!("Failed to connect to backend for history prefetch");
    }

    // Initialize Wayland for layer shell capture
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue): (GlobalList, EventQueue<State>) =
        registry_queue_init::<State>(&conn)?;

    queue.roundtrip(&mut state)?;

    // Initialize Wayland protocols
    init_wayland_protocols(&globals, &queue, &mut state)?;

    // Create capture surfaces for mouse coordinate detection
    setup_capture_layer(&mut state, &queue)?;

    // Main event loop (reuse existing implementation)
    run_main_event_loop(&mut state, &mut queue).await
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
        return Err("wl_compositor not available".into());
    }

    // Bind zwlr_layer_shell_v1
    if let Ok(layer_shell) =
        globals.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(&queue.handle(), 4..=4, ())
    {
        state.layer_shell = Some(layer_shell);
    } else {
        return Err("zwlr_layer_shell_v1 not available".into());
    }

    // Bind wl_seat
    if let Ok(seat) = globals.bind::<wl_seat::WlSeat, _, _>(&queue.handle(), 1..=1, ()) {
        state.seat = Some(seat);
    } else {
        return Err("wl_seat not available".into());
    }

    // Bind wp_viewporter
    if let Ok(viewporter) = globals.bind::<wp_viewporter::WpViewporter, _, _>(&queue.handle(), 1..=1, ()) {
        state.viewporter = Some(viewporter);
    } else {
        eprintln!("wp_viewporter not available");
    }

    // Bind wp_single_pixel_buffer_manager_v1
    if let Ok(single_pixel_buffer_manager) =
        globals.bind::<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1, _, _>(
            &queue.handle(),
            1..=1,
            (),
        )
    {
        state.single_pixel_buffer_manager = Some(single_pixel_buffer_manager);
    } else {
        eprintln!("wp_single_pixel_buffer_manager_v1 not available");
    }

    // Bind virtual_pointer_manager_v1
    if let Ok(virtual_pointer_manager) =
        globals.bind::<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, _, _>(
            &queue.handle(),
            1..=1,
            (),
        )
    {
        if let Some(seat) = &state.seat {
            let virtual_pointer =
                virtual_pointer_manager.create_virtual_pointer(Some(seat), &queue.handle(), ());
            state.virtual_pointer = Some(virtual_pointer);
        }
        state.virtual_pointer_manager = Some(virtual_pointer_manager);
    } else {
        eprintln!("zwlr_virtual_pointer_manager_v1 not available");
    }

    Ok(())
}

fn setup_capture_layer(state: &mut State, queue: &EventQueue<State>) -> Result<(), Box<dyn std::error::Error>> {
    let compositor = state
        .compositor
        .as_ref()
        .expect("Compositor not initialized");

    let capture_surface = compositor.create_surface(&queue.handle(), ());
    let update_surface = compositor.create_surface(&queue.handle(), ());

    state.capture_surface = Some(capture_surface.clone());
    state.update_surface = Some(update_surface.clone());

    let layer_shell = state
        .layer_shell
        .as_ref()
        .expect("Layer Shell not initialized");

    // Create single-pixel buffers via the wp_single_pixel_buffer_manager (no SHM file needed)
    let spbm = state
        .single_pixel_buffer_manager
        .as_ref()
        .expect("single_pixel_buffer_manager not initialized");

    // Single shared buffer (transparent) reused for both capture and update layer surfaces
    let transparent_buffer = spbm.create_u32_rgba_buffer(0x00, 0x00, 0x00, 0x00, &queue.handle(), ());
    state.transparent_buffer = Some(transparent_buffer);

    let capture_layer_surface = layer_shell.get_layer_surface(
        &capture_surface,
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
    capture_surface.commit();

    Ok(())
}
