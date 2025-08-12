use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport;

use crate::state::State;

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
                let Some(_manager) = &state.single_pixel_buffer_manager else {
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
                let viewport: wp_viewport::WpViewport =
                    viewporter.get_viewport(&capture_surface, qhandle, ());
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
                if !state.capture_layer_ready {
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
