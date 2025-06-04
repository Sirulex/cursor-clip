use wayland_client::{
    globals::{registry_queue_init, GlobalList, GlobalListContents},
    protocol::{wl_compositor, wl_registry}, Connection, Dispatch, EventQueue, QueueHandle
};

use smithay_client_toolkit::shell::wlr_layer::{LayerShell, LayerSurface,LayerSurfaceConfigure,Anchor};

struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<LayerShell>,
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
    if let Ok(layer_shell) = globals.bind::<LayerShell, _, _>(&queue.handle(), 4..=4, ()) {
        state.layer_shell = Some(layer_shell);
    } else {
        eprintln!("zwlr_layer_shell_v1 not available");
    }
   
    // Dispatch initial events
    //queue.blocking_dispatch(&mut state).unwrap();
    // Use state.compositor and state.layer_shell as needed
    println!("Wayland client initialized successfully.");
    println!("Compositor: {:?}", state.compositor);
}
