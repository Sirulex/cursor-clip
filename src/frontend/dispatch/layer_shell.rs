use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport;

use crate::frontend::frontend_state::State;
use crate::frontend::dispatch::frame_callback::FrameCallbackData;
use log::debug;

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
                debug!("Layer surface configured: {}x{}", width, height);

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

                if let Some(buffer) = &state.transparent_buffer {
                    capture_surface.attach(Some(buffer), 0, 0);
                }

                let Some(viewporter) = &state.viewporter else {
                    debug!("Viewporter not available");
                    return;
                };
                let viewport: wp_viewport::WpViewport =
                    viewporter.get_viewport(capture_surface, qhandle, ());
                viewport.set_destination(width as i32, height as i32);

                // 1. Create a region object from the compositor.
                let region = compositor.create_region(qhandle, ());
                // 2. Add a rectangle to the region that covers the entire surface.
                region.add(0, 0, width as i32, height as i32);
                // 3. Set this as the input region for the surface.
                capture_surface.set_input_region(Some(&region));
                // 4. The surface now holds the state of the region. We can
                //    destroy our client-side handle to it.
                region.destroy();

                // Mark the entire surface as damaged
                capture_surface.damage(0, 0, width as i32, height as i32);

                if !state.capture_layer_ready {
                    state.capture_layer_ready = true; // Set flag to indicate layer is ready
                    debug!("Setting capture_layer_ready to true");
                    
                    // Create frame callback for capture layer to know when it's shown
                    let frame_callback = capture_surface.frame(qhandle, FrameCallbackData::CaptureLayer);
                    state.capture_frame_callback = Some(frame_callback);
                    
                    capture_surface.commit();
                } else {
                    // This is the update layer surface configuration
                    debug!("Update layer surface configured");
                    
                    // Create frame callback for update layer to know when it's shown
                    if let Some(update_surface) = &state.update_surface {
                        if let Some(update_buffer) = &state.transparent_buffer {
                            update_surface.attach(Some(update_buffer), 0, 0);
                        }
                        
                        // Create viewport for update surface
                        if let Some(viewporter) = &state.viewporter {
                            let update_viewport = viewporter.get_viewport(update_surface, qhandle, ());
                            update_viewport.set_destination(width as i32, height as i32);
                        }
                        
                        // Create input region for update surface
                        let update_region = compositor.create_region(qhandle, ());
                        update_region.add(0, 0, width as i32, height as i32);
                        update_surface.set_input_region(Some(&update_region));
                        update_region.destroy();
                        
                        // Mark damage
                        update_surface.damage(0, 0, width as i32, height as i32);
                        
                        // Create frame callback for update surface
                        let update_frame_callback = update_surface.frame(qhandle, FrameCallbackData::UpdateLayer);
                        state.update_frame_callback = Some(update_frame_callback);
                        
                        update_surface.commit();
                    }
                }
            }

            zwlr_layer_surface_v1::Event::Closed => {
                debug!("Layer surface was closed");
            }

            _ => {}
        }
    }
}
