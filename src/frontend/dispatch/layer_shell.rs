use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_surface_v1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport;

use crate::frontend::frontend_state::State;
use crate::frontend::dispatch::frame_callback::FrameCallbackData;
use log::debug;

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

/// Destroy capture/update layer resources and underlying Wayland objects.
pub fn cleanup_capture_layer(state: &mut State) {
    debug!("Cleaning up capture/update layer resources");

    // Destroy update layer resources first if present
    if let Some(update_layer_surface) = state.update_layer_surface.take() {
        update_layer_surface.destroy();
        debug!("Update layer surface destroyed");
    }
    if let Some(update_surface) = state.update_surface.take() {
        update_surface.destroy();
        debug!("Update surface destroyed");
    }
    state.update_frame_callback = None;

    // Destroy capture layer surface
    if let Some(capture_layer_surface) = state.capture_layer_surface.take() {
        capture_layer_surface.destroy();
        debug!("Capture layer surface destroyed");
    }

    // Destroy underlying surfaces and buffers
    if let Some(capture_surface) = state.capture_surface.take() {
        capture_surface.destroy();
        debug!("Capture wl_surface destroyed");
    }

    if let Some(buffer) = state.transparent_buffer.take() {
        buffer.destroy();
        debug!("Transparent buffer destroyed");
    }

    state.capture_layer_clicked = false;
}

/// Destroy only the update layer resources (used after minimal frame delay).
pub fn cleanup_update_layer(state: &mut State) {
    debug!("Cleaning up update layer resources");

    // Destroy the update layer surface if it exists
    if let Some(update_layer_surface) = state.update_layer_surface.take() {
        debug!("Destroying update layer surface");
        update_layer_surface.destroy();
    }

    // Clean up the update surface
    if let Some(update_surface) = state.update_surface.take() {
        debug!("Destroying update surface");
        update_surface.destroy();
    }

    // Clear the update frame callback reference (callback-resources are auto-cleaned)
    if state.update_frame_callback.is_some() {
        debug!("Clearing update frame callback reference");
        state.update_frame_callback = None;
    }

    debug!("Update layer cleanup completed");
}
