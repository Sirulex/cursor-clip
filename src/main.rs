use wayland_client::{
    globals::{registry_queue_init, GlobalList, GlobalListContents},
    protocol::{wl_compositor, wl_registry, wl_surface}, Connection, Dispatch, EventQueue, QueueHandle
};

use smithay_client_toolkit::{
    reexports::protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1,
    reexports::protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1,
    shell::wlr_layer::{Anchor,LayerShell, LayerSurface, LayerSurfaceConfigure}
};

struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut State,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // React to dynamic global events if needed
        println!("Received registry event: {:?}", _event);
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for State {
    fn event(
        _state: &mut State,
        _compositor: &wl_compositor::WlCompositor,
        _event: wl_compositor::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle compositor events if needed
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for State {
    fn event(
        _state: &mut State,
        _layer_shell: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _event: zwlr_layer_shell_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle layer shell events if needed
    }
}

// Add implementation for wl_surface
impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn event(
        _state: &mut State,
        _surface: &wl_surface::WlSurface,
        _event: wl_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle surface events if needed
    }
}

// Add implementation for zwlr_layer_surface_v1
impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        _state: &mut State,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                // Acknowledge the configure event
                layer_surface.ack_configure(serial);
                println!("Layer surface configured with size: {}x{}", width, height);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                println!("Layer surface was closed");
                // You might want to exit the application here
            }
            _ => {}
        }
    }
}


fn main() {
    let conn = Connection::connect_to_env().unwrap();
    let (globals, mut queue): (GlobalList, EventQueue<State>) = registry_queue_init::<State>(&conn).unwrap();

    // Create initial state
    let mut state = State {
        compositor: None,
        layer_shell: None,
    };


    queue.roundtrip(&mut state).unwrap();

    // Bind wl_compositor
    if let Ok(compositor) = globals.bind::<wl_compositor::WlCompositor, _, _>(&queue.handle(), 4..=5, ()) {
        state.compositor = Some(compositor);
    } else {
        eprintln!("wl_compositor not available");
    }

    // Bind zwlr_layer_shell_v1
    if let Ok(layer_shell) = globals.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(&queue.handle(), 4..=4, ()) {
        state.layer_shell = Some(layer_shell);
    } else {
        eprintln!("zwlr_layer_shell_v1 not available");
    }
   
    // Dispatch initial events
    //queue.blocking_dispatch(&mut state).unwrap();
    // Use state.compositor and state.layer_shell as needed
    println!("Wayland client initialized successfully.");
    println!("Compositor: {:?}", state.compositor);
    println!("Layer Shell: {:?}", state.layer_shell);

    let compositor = state.compositor.as_ref().expect("Compositor not initialized");
    let surface = compositor.create_surface(&queue.handle(), ());
    
    let layer_shell = state.layer_shell.as_ref().expect("Layer Shell not initialized");
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        None, // output (None means all outputs)
        zwlr_layer_shell_v1::Layer::Overlay, // layer type
        "cursor-clip".to_string(), // namespace
        &queue.handle(),
        (), // user data
    );

    // Configure the layer surface
    layer_surface.set_size(100, 100); // Width and height in pixels
    layer_surface.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left | zwlr_layer_surface_v1::Anchor::Right | zwlr_layer_surface_v1::Anchor::Bottom
    ); // Anchor to all edges
    layer_surface.set_exclusive_zone(-1); // -1 means don't reserve space
    
    // Commit the surface to apply the changes
    surface.commit();
    
    // Dispatch events to handle surface configuration
    queue.roundtrip(&mut state).unwrap();
    
    // Keep the application running
    loop {
        queue.blocking_dispatch(&mut state).unwrap();
    }


}
