use wayland_client::{
    globals::{registry_queue_init, GlobalList, GlobalListContents}, protocol::{wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface}, Connection, Dispatch, EventQueue, QueueHandle
};

use wayland_protocols_wlr::
    layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};

//use smithay_client_toolkit::{
//    shm::{slot::SlotPool, Shm}
//};

struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    shm: Option<wl_shm::WlShm>,
    pool: Option<wl_shm_pool::WlShmPool>,
    buffer: Option<wl_buffer::WlBuffer>,
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

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(
        _state: &mut State,
        _shm: &wl_shm::WlShm,
        _event: wl_shm::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle shm events if needed
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    fn event(
        _state: &mut State,
        _pool: &wl_shm_pool::WlShmPool,
        _event: wl_shm_pool::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle shm pool events if needed
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(
        _state: &mut State,
        _buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        if let wl_buffer::Event::Release = event {
            // Buffer is no longer used by the compositor
            println!("Buffer released by compositor");
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
        shm: None,
        pool: None,
        buffer: None,
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

    // Initialize SHM
    if let Ok(shm) = globals.bind::<wl_shm::WlShm, _, _>(&queue.handle(), 1..=1, ()) {
        state.shm = Some(shm);
    } else {
        eprintln!("wl_shm not available");
    }

    // Initialize the SlotPool from SCTK

   
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
        zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left | 
        zwlr_layer_surface_v1::Anchor::Right | zwlr_layer_surface_v1::Anchor::Bottom
    ); // Anchor to all edges
    layer_surface.set_exclusive_zone(-1); // -1 means don't reserve space
    
    // Commit the surface to apply the changes
    surface.commit();
    
    // Dispatch events to handle surface configuration
    queue.roundtrip(&mut state).unwrap();

    // Create a buffer with red color
    let width = 100;
    let height = 100;
    let stride = width * 4; // 4 bytes per pixel (ARGB8888)
    let size = stride * height;

    // Get a buffer from the pool using SCTK
    //let mut pool = state.pool.as_mut().expect("Memory pool not initialized");
    
    // Create a buffer from the pool
    //let (buffer, canvas) = pool.create_buffer(
    //    width as i32,
    //    height as i32,
    //    stride as i32,
    //    wl_shm::Format::Argb8888,
    //).expect("Failed to create buffer");

    // Fill the buffer with red color
    //for pixel in canvas.chunks_exact_mut(4) {
    //    pixel[0] = 0x00; // B
    //    pixel[1] = 0x00; // G
    //    pixel[2] = 0xFF; // R
    //    pixel[3] = 0xFF; // A (fully opaque)
    //}
    //
    //// Attach the buffer to the surface
    //surface.attach(Some(&buffer), 0, 0);
    //
    //// Mark the entire surface as damaged (needs redrawing)
    //surface.damage(0, 0, width as i32, height as i32);
    //
    //// Commit the surface to apply changes
    //surface.commit();
    let fd = {
        use std::os::fd::{FromRawFd, AsRawFd};
        use memmap2::MmapOptions;
        use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
        use nix::unistd::ftruncate;

        // Create anonymous file
        let mfd = memfd_create("buffer", MemFdCreateFlag::MFD_CLOEXEC)
            .expect("Failed to create memfd");
        
        // Set size
        ftruncate(mfd.as_raw_fd(), size as i64)
            .expect("Failed to set memfd size");

        mfd
    };

    //let shm = state.shm.as_ref().expect("SHM not initialized");

    
    
    // Keep the application running
    loop {
        queue.blocking_dispatch(&mut state).unwrap();
    }
}