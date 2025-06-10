use wayland_client::{
    globals::{registry_queue_init, GlobalList, GlobalListContents}, protocol::{wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface, wl_seat, wl_pointer}, Connection, Dispatch, EventQueue, QueueHandle
};

use wayland_protocols_wlr::
    layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::os::fd::BorrowedFd;
use memmap2::{MmapMut,MmapOptions};

//use smithay_client_toolkit::{
//    shm::{slot::SlotPool, Shm}
//};


struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pointer: Option<wl_pointer::WlPointer>,
    shm: Option<wl_shm::WlShm>,
    pool: Option<wl_shm_pool::WlShmPool>,
    buffer: Option<wl_buffer::WlBuffer>,
}


impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut State,
        _proxy: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // React to dynamic global events if needed
        //println!("Received registry event: {:?}", _event);
        if let wl_registry::Event::Global { name, interface, version } = event {
            println!("[{}] {} (v{})", name, interface, version);
        }
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

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _state: &mut State,
        _seat: &wl_seat::WlSeat,
        _event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle seat events if needed
        if let wayland_client::protocol::wl_seat::Event::Capabilities { capabilities } = event {
            if capabilities.contains(wayland_client::protocol::wl_seat::Capability::Pointer) {
                let pointer = seat.get_pointer(qh, ());
                self.pointer = Some(pointer);
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        _state: &mut State,
        _pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        match event {
            wl_pointer::Event::Enter { serial: _ ,surface, surface_x,surface_y} => {
                println!("Pointer entered surface: {:?} at ({}, {})", surface, surface_x, surface_y);
            }
            wl_pointer::Event::Leave { serial: _, surface } => {
                println!("Pointer left surface: {:?}", surface);
            }
            wl_pointer::Event::Motion { time, surface_x, surface_y } => {
                println!("Pointer moved to ({}, {}) at time {}", surface_x, surface_y, time);
            }
            wl_pointer::Event::Button { serial: _, time, button, state } => {
                println!("Pointer button {:?} at time {}: {:?}", button, time, state);
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
        pointer: None,
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

    // Bind wl_seat
    if let Ok(seat) = globals.bind::<wl_seat::WlSeat, _, _>(&queue.handle(), 1..=1, ()) {
        // You can use the seat for input handling
        println!("Bound to wl_seat: {:?}", seat);
    } else {
        eprintln!("wl_seat not available");
    }

    // Bind wl_pointer
    //if let Ok(pointer) = globals.bind::<wl_pointer::WlPointer, _, _>(&queue.handle(), 1..=4, ()) {
    //    state.pointer = Some(pointer);
    //} else {
    //    eprintln!("wl_pointer not available");
    //}

   
    println!("Wayland client initialized successfully.");
    println!("Compositor: {:?}", state.compositor);
    println!("Layer Shell: {:?}", state.layer_shell);
    println!("Pointer: {:?}", state.pointer);

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

    let path = "/dev/shm/wayland-shared-buffer";
    let file = OpenOptions::new()
    .read(true)
    .write(true)
    .create(true)
    .open(path)
    .expect("Failed to open shared memory file");

    file.set_len(size as u64).expect("Failed to set file size");

    let mut mmap: MmapMut = unsafe {
    MmapOptions::new()
        .len(size)
        .map_mut(&file)
        .expect("Failed to map the file")
    };

    for pixel in mmap.chunks_exact_mut(4) {
    pixel[0] = 0xFF; // Blue
    pixel[1] = 0x00; // Green
    pixel[2] = 0x00; // Red
    pixel[3] = 0xFF; // Alpha
}

    let fd = file.as_raw_fd();
    let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
    // Create a pool from the file descriptor
    let shm = state.shm.as_ref().expect("SHM not initialized");
    let pool = shm.create_pool(borrowed_fd, size as i32, &queue.handle(), ());
    
    // Create a buffer from the pool
    let buffer = pool.create_buffer(
        0, width as i32, height as i32,
        stride as i32, wl_shm::Format::Argb8888,
        &queue.handle(), ()
    );
    

    // Save the pool and buffer in state
    state.pool = Some(pool);
    state.buffer = Some(buffer.clone());

    // Attach the buffer to the surface
    surface.attach(Some(&buffer), 0, 0);
    
    // Mark the entire surface as damaged (needs redrawing)
    surface.damage(0, 0, width as i32, height as i32);
    
    // Commit the surface to apply changes
    surface.commit();

    // Keep the application running
    loop {
        queue.blocking_dispatch(&mut state).unwrap();
    }
}