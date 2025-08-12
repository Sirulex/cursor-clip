use wayland_client::{
    Connection, EventQueue,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_compositor, wl_seat, wl_shm},
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
    xdg::shell::client::xdg_wm_base,
};

mod state;
mod buffer;
mod dispatch;

use state::State;

fn main() {
    let conn = Connection::connect_to_env().unwrap();
    let (globals, mut queue): (GlobalList, EventQueue<State>) =
        registry_queue_init::<State>(&conn).unwrap();

    // Create initial state
    let mut state = State::new();

    queue.roundtrip(&mut state).unwrap();

    // Bind wl_compositor
    if let Ok(compositor) =
        globals.bind::<wl_compositor::WlCompositor, _, _>(&queue.handle(), 4..=5, ())
    {
        state.compositor = Some(compositor);
    } else {
        eprintln!("wl_compositor not available");
    }

    // Bind zwlr_layer_shell_v1
    if let Ok(layer_shell) =
        globals.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(&queue.handle(), 4..=4, ())
    {
        state.layer_shell = Some(layer_shell);
    } else {
        eprintln!("zwlr_layer_shell_v1 not available");
    }

    if let Ok(xdg_shell) = globals.bind::<xdg_wm_base::XdgWmBase, _, _>(&queue.handle(), 1..=1, ())
    {
        state.xdg_wm_base = Some(xdg_shell);
    } else {
        eprintln!("xdg_wm_base not available");
    }

    // Initialize SHM
    if let Ok(shm) = globals.bind::<wl_shm::WlShm, _, _>(&queue.handle(), 1..=1, ()) {
        state.shm = Some(shm);
    } else {
        eprintln!("wl_shm not available");
    }

    // Bind wl_seat
    if let Ok(seat) = globals.bind::<wl_seat::WlSeat, _, _>(&queue.handle(), 1..=1, ()) {
        state.seat = Some(seat);
    } else {
        eprintln!("wl_seat not available");
    }

    //bind wp_viewporter
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

    //bind virtual_pointer_manager_v1
    if let Ok(virtual_pointer_manager) =
        globals.bind::<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, _, _>(
            &queue.handle(),
            1..=1,
            (),
        )
    {
        // Create a virtual pointer for synthetic input
        if let Some(seat) = &state.seat {
            let virtual_pointer =
                virtual_pointer_manager.create_virtual_pointer(Some(seat), &queue.handle(), ());
            state.virtual_pointer = Some(virtual_pointer);
        }
        state.virtual_pointer_manager = Some(virtual_pointer_manager);
    } else {
        eprintln!("zwlr_virtual_pointer_manager_v1 not available");
    }

    let compositor = state
        .compositor
        .as_ref()
        .expect("Compositor not initialized");

    let capture_surface = compositor.create_surface(&queue.handle(), ());
    let update_surface = compositor.create_surface(&queue.handle(), ());

    state.capture_surface = Some(capture_surface.clone()); //valid as surface is basically a reference to the proxy object
    state.update_surface = Some(update_surface.clone());

    let xdg_wm_base = state
        .xdg_wm_base
        .as_ref()
        .expect("XDG WM Base not initialized");

    let xdg_surface=xdg_wm_base.get_xdg_surface( //only layer shell or xdg shell can be used at the same time
        &update_surface,
        &queue.handle(),
        (),
    ); // Create an xdg surface
    let xdg_toplevel = xdg_surface.get_toplevel(&queue.handle(), ()); // Create a toplevel surface
    xdg_toplevel.set_title("Cursor Clip".to_string()); // Set the title of the toplevel surface
    xdg_toplevel.set_app_id("com.sirulex.cursor_clip".to_string()); // Set the app ID

    let layer_shell = state
        .layer_shell
        .as_ref()
        .expect("Layer Shell not initialized");

    // Create buffers using the helper function
    let shm = state.shm.as_ref().expect("SHM not initialized");
    let (pool, capture_buffer) = buffer::create_shared_buffer(shm, 1, 1, &queue.handle())
        .expect("Failed to create shared buffer");

    let update_buffer = capture_buffer.clone();
    state.update_buffer = Some(update_buffer);

    // Save the pool and buffer in state
    state.pool = Some(pool);
    state.capture_buffer = Some(capture_buffer.clone());

    let capture_layer_surface = layer_shell.get_layer_surface(
        &capture_surface,
        None,                                // output (None means all outputs)
        zwlr_layer_shell_v1::Layer::Overlay, // layer type
        "cursor-clip".to_string(),           // namespace
        &queue.handle(),
        (), // user data
    );

    // Configure the layer surface
    //layer_surface.set_size(200, 300); // Width and height in pixels (no need due to autoscaling via viewporter)
    capture_layer_surface.set_exclusive_zone(-1); // -1 -> don't reserve space
    capture_layer_surface.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right
            | zwlr_layer_surface_v1::Anchor::Bottom,
    ); // Anchor to all edges

    capture_layer_surface.set_margin(100, 100 , 100, 100);
    

    
    //let update_layer_surface = layer_shell.get_layer_surface(
    //    &update_surface,
    //    None,                                // output (None means all outputs)
    //    zwlr_layer_shell_v1::Layer::Overlay, // layer type
    //    "cursor-clip".to_string(),           // namespace
    //    &queue.handle(),
    //    (), // user data
    //);
    //
    //update_layer_surface.set_exclusive_zone(-1); // -1 -> don't reserve space
    //update_layer_surface.set_anchor(
    //    zwlr_layer_surface_v1::Anchor::Top
    //        | zwlr_layer_surface_v1::Anchor::Left
    //        | zwlr_layer_surface_v1::Anchor::Right
    //        | zwlr_layer_surface_v1::Anchor::Bottom,
    //); // Anchor to all edges

    capture_surface.commit();
    
    // Keep the application running
    while !state.coords_received {
        queue.blocking_dispatch(&mut state).unwrap();
    }
}
