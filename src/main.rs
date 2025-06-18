use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
    globals::{GlobalList, GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer, wl_compositor, wl_pointer, wl_region, wl_registry, wl_seat, wl_shm, wl_shm_pool,
        wl_surface,
    },
};

use wayland_protocols_wlr::{
    layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
    virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1},
};

use wayland_protocols::{
    wp::{
        single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1,
        viewporter::client::{wp_viewport, wp_viewporter},
    },
    xdg::{shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base}},
};

//use smithay_client_toolkit::{
//    shm::{slot::SlotPool, Shm}
//};

use memmap2::{MmapMut, MmapOptions};
use std::fs::OpenOptions;
use std::os::fd::BorrowedFd;
use std::os::unix::io::AsRawFd;

struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    xdg_wm_base: Option<xdg_wm_base::XdgWmBase>,
    pointer: Option<wl_pointer::WlPointer>,
    shm: Option<wl_shm::WlShm>,
    pool: Option<wl_shm_pool::WlShmPool>,
    seat: Option<wl_seat::WlSeat>,
    single_pixel_buffer_manager:
    Option<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    virtual_pointer_manager: Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1>,
    virtual_pointer: Option<zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1>,
    coords_received: bool,
    capture_layer_ready: bool,
    capture_surface: Option<wl_surface::WlSurface>,
    capture_buffer: Option<wl_buffer::WlBuffer>,
    update_surface: Option<wl_surface::WlSurface>,
    update_buffer: Option<wl_buffer::WlBuffer>,
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
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
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

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn event(
        _state: &mut State,
        _xdg_wm_base: &xdg_wm_base::XdgWmBase,
        _event: xdg_wm_base::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle xdg_wm_base events if needed
        match _event {
            xdg_wm_base::Event::Ping { serial } => {
                // Respond to ping events
                _xdg_wm_base.pong(serial);
                println!("Received ping with serial: {}", serial);
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for State {
    fn event(
        _state: &mut State,
        _xdg_surface: &xdg_surface::XdgSurface,
        _event: xdg_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle xdg_surface events if needed
        match _event {
            xdg_surface::Event::Configure { serial } => {
                // Acknowledge the configure event
                _xdg_surface.ack_configure(serial);
                println!("XDG surface configured with serial: {}", serial);
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for State {
    fn event(
        state: &mut State,
        _xdg_toplevel: &xdg_toplevel::XdgToplevel,
        _event: xdg_toplevel::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<State>,
    ) {
        // Handle xdg_toplevel events if needed
        match _event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                // Handle toplevel configuration
                let Some(update_surface) = &state.update_surface else {
                    return;
                };

                if let Some(update_buffer) = &state.update_buffer {
                    update_surface.attach(Some(update_buffer), 0, 0);
                }

                let Some(viewporter) = &state.viewporter else {
                    eprintln!("Viewporter not available");
                    return;
                };
                let viewport: wp_viewport::WpViewport =viewporter.get_viewport(&update_surface, qhandle, ());

                viewport.set_destination(width as i32, height as i32);

                // Mark the entire surface as damaged
                update_surface.damage(0, 0, width as i32, height as i32);

                update_surface.commit();
            }
            xdg_toplevel::Event::Close => {
                // Handle close event
                println!("XDG toplevel closed");
            }
            _ => {}
        }
    }
}

// Add implementation for zwlr_layer_surface_v1
impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut State,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<State>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surface.ack_configure(serial);
                println!("Layer surface configured: {}x{}", width, height);

                // Ensure we have the necessary objects
                let Some(manager) = &state.single_pixel_buffer_manager else {
                    return;
                };

                let Some(compositor) = &state.compositor else {
                    return;
                };

                let Some(capture_surface) = &state.capture_surface else {
                    return;
                };

                if let Some(buffer) = &state.capture_buffer {
                    capture_surface.attach(Some(buffer), 0, 0);
                }

                let Some(viewporter) = &state.viewporter else {
                    eprintln!("Viewporter not available");
                    return;
                };
                let viewport: wp_viewport::WpViewport =viewporter.get_viewport(&capture_surface, qhandle, ());
                viewport.set_destination(width as i32, height as i32);

                // Create and attach the buffer
                //let buffer = manager.create_u32_rgba_buffer(0xFF, 0x00, 0x00, 0x80, qhandle, ()); //manual buffer alloc
                //surface.attach(Some(&buffer), 0, 0);

                // 1. Create a region object from the compositor.
                let region = compositor.create_region(qhandle, ());
                // 2. Add a rectangle to the region that covers the entire surface.
                region.add(0, 0, width as i32, height as i32);
                // 3. Set this as the input region for the surface.
                capture_surface.set_input_region(Some(&region));
                // 4. The surface now holds the state of the region. We can
                //    destroy our client-side handle to it.
                region.destroy();
                // --- END OF NEW PART ---
   
                // Mark the entire surface as damaged
                capture_surface.damage(0, 0, width as i32, height as i32);

                // Commit all pending state changes at once:
                // - The attached buffer
                // - The new input region-
                // - The damage
                if (!state.capture_layer_ready) {
                    state.capture_layer_ready = true; // Set flag to indicate layer is ready
                    println!("setting bool to true (capture_layer_ready)");
                } else {
                    println!("capture_layer_ready is already true, now at update layer surface");
                }

                capture_surface.commit();
            }

            zwlr_layer_surface_v1::Event::Closed => {
                println!("Layer surface was closed");
            }

            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut State,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<State>,
    ) {
        println!("WL Seat event received yay: {:?}", event);
        if let wl_seat::Event::Capabilities {
            capabilities: cap_event_enum,
        } = event
        {
            //detangle Capabilities enum

            if let WEnum::Value(capabilities) = cap_event_enum {
                println!("Pointer capabilities detected.");

                if capabilities.contains(wl_seat::Capability::Pointer) {
                    //no pattern matching as wl_seat::Capability is a bitfield
                    let pointer = seat.get_pointer(qhandle, ());
                    state.pointer = Some(pointer);
                    println!("Pointer capabilities detected, pointer created.");
                } else {
                    println!("No pointer capabilities detected.");
                }
            } else {
                println!("Unknown capability enumerator");
            }
        }
        //impl release events todo
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut State,
        _pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        println!("WL Pointer event received");
        match event {
            wl_pointer::Event::Enter {
                serial: _,
                surface,
                surface_x,
                surface_y,
            } => {
                println!(
                    "Pointer entered surface: {:?} at ({}, {})",
                    surface, surface_x, surface_y
                );
                state.coords_received = true; // Set flag when coordinates are received
            }
            wl_pointer::Event::Leave { serial: _, surface } => {
                println!("Pointer left surface: {:?}", surface);
            }
            wl_pointer::Event::Motion {
                time,
                surface_x,
                surface_y,
            } => {
                println!(
                    "Pointer moved to ({}, {}) at time {}",
                    surface_x, surface_y, time
                );
            }
            wl_pointer::Event::Button {
                serial: _,
                time,
                button,
                state,
            } => {
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

impl Dispatch<wp_viewporter::WpViewporter, ()> for State {
    fn event(
        _state: &mut State,
        _viewporter: &wp_viewporter::WpViewporter,
        _event: wp_viewporter::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle viewporter events if needed
    }
}

impl Dispatch<wp_viewport::WpViewport, ()> for State {
    fn event(
        _state: &mut State,
        _viewport: &wp_viewport::WpViewport,
        _event: wp_viewport::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle viewport events if needed
    }
}

impl Dispatch<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1, ()> for State {
    fn event(
        _state: &mut State,
        _manager: &wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
        _event: wp_single_pixel_buffer_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle single pixel buffer manager events if needed
    }
}

impl Dispatch<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(
        _state: &mut State,
        _manager: &zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
        _event: zwlr_virtual_pointer_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle virtual pointer manager events if needed
    }
}

impl Dispatch<zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1, ()> for State {
    fn event(
        _state: &mut State,
        _virtual_pointer: &zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
        _event: zwlr_virtual_pointer_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle virtual pointer events if needed
    }
}

impl Dispatch<wl_region::WlRegion, ()> for State {
    fn event(
        _state: &mut State,
        _region: &wl_region::WlRegion,
        _event: wl_region::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle region events if needed
    }
}

fn main() {
    let conn = Connection::connect_to_env().unwrap();
    let (globals, mut queue): (GlobalList, EventQueue<State>) =
        registry_queue_init::<State>(&conn).unwrap();

    // Create initial state
    let mut state = State {
        compositor: None,
        layer_shell: None,
        xdg_wm_base: None,
        pointer: None,
        shm: None,
        seat: None,
        pool: None,
        single_pixel_buffer_manager: None,
        viewporter: None,
        virtual_pointer_manager: None,
        virtual_pointer: None,
        coords_received: false,
        capture_layer_ready: false,
        capture_surface: None,
        capture_buffer: None,
        update_surface: None,
        update_buffer: None,
    };

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
    ); // Create an xdg surfac
    let xdg_toplevel = xdg_surface.get_toplevel(&queue.handle(), ()); // Create a toplevel surface
    xdg_toplevel.set_title("Cursor Clip".to_string()); // Set the title of the toplevel surface
    xdg_toplevel.set_app_id("com.sirulex.cursor_clip".to_string()); // Set the app ID

    let layer_shell = state
        .layer_shell
        .as_ref()
        .expect("Layer Shell not initialized");

    // Create a buffer with blue color
    let buf_width = 1;
    let buf_height = 1;
    let stride = buf_width * 4; // 4 bytes per pixel (ARGB8888)
    let size = stride * buf_height;

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
        pixel[1] = 0x00; // Green-
        pixel[2] = 0x00; // Red
        pixel[3] = 0xFF; // Alpha
    }

    let fd = file.as_raw_fd();
    let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
    // Create a pool from the file descriptor
    let shm = state.shm.as_ref().expect("SHM not initialized");
    let pool = shm.create_pool(borrowed_fd, size as i32, &queue.handle(), ());

    // Create a buffer from the pool
    let capture_buffer = pool.create_buffer(
        0,
        buf_width as i32,
        buf_height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        &queue.handle(),
        (),
    );

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

    //capture_surface.commit();
    
    update_surface.commit();
    //update_surface.destroy();;
    
    //update_surface.destroy();
    //capture_layer_surface.destroy();
    //update_layer_surface.destroy();
    // Keep the application running
    while true {//!state.coords_received {
        queue.blocking_dispatch(&mut state).unwrap();
    }
}
