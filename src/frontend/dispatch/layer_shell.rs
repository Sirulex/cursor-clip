use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1;

use crate::frontend::dispatch::frame_callback::FrameCallbackData;
use crate::frontend::frontend_state::State;
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

                // Check if this is the capture layer surface
                if Some(layer_surface) == state.capture_layer_surface.as_ref() {
                    let Some(capture_surface) = &state.capture_surface else {
                        return;
                    };
                    if let Some(buffer) = &state.transparent_buffer {
                        capture_surface.attach(Some(buffer), 0, 0);
                    }

                    if state.capture_viewport.is_none() {
                        if let Some(viewporter) = &state.viewporter {
                            let viewport = viewporter.get_viewport(capture_surface, qhandle, ());
                            state.capture_viewport = Some(viewport);
                        }
                    }
                    if let Some(viewport) = &state.capture_viewport {
                        viewport.set_destination(width as i32, height as i32);
                    }
                    capture_surface.damage(0, 0, width as i32, height as i32);

                    if !state.capture_layer_ready {
                        state.capture_layer_ready = true;
                        debug!("Setting capture_layer_ready to true");
                        let frame_callback =
                            capture_surface.frame(qhandle, FrameCallbackData::CaptureLayer);
                        state.capture_frame_callback = Some(frame_callback);
                    }
                    capture_surface.commit();
                }

                // Check if this is the update layer surface
                if Some(layer_surface) == state.update_layer_surface.as_ref() {
                    let Some(update_surface) = &state.update_surface else {
                        return;
                    };
                    if let Some(buffer) = &state.transparent_buffer {
                        update_surface.attach(Some(buffer), 0, 0);
                    }

                    if state.update_viewport.is_none() {
                        if let Some(viewporter) = &state.viewporter {
                            let viewport = viewporter.get_viewport(update_surface, qhandle, ());
                            state.update_viewport = Some(viewport);
                        }
                    }
                    if let Some(viewport) = &state.update_viewport {
                        viewport.set_destination(width as i32, height as i32);
                    }
                    update_surface.damage(0, 0, width as i32, height as i32);

                    let frame_callback =
                        update_surface.frame(qhandle, FrameCallbackData::UpdateLayer);
                    state.update_frame_callback = Some(frame_callback);

                    update_surface.commit();
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

    if let Some(viewport) = state.capture_viewport.take() {
        viewport.destroy();
    }
    if let Some(viewport) = state.update_viewport.take() {
        viewport.destroy();
    }

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

    // Destroy SHM pool if it was used (after buffer is gone)
    if let Some(pool) = state.shm_pool.take() {
        pool.destroy();
        debug!("SHM pool destroyed");
    }
    // Drop SHM file handle if present
    if state.shm_file.take().is_some() {
        debug!("SHM file dropped");
    }

    state.capture_layer_clicked = false;
}

/// Destroy only the update layer resources (used after minimal frame delay).
pub fn cleanup_update_layer(state: &mut State) {
    debug!("Cleaning up update layer resources");

    if let Some(viewport) = state.update_viewport.take() {
        viewport.destroy();
    }
    if let Some(update_layer_surface) = state.update_layer_surface.take() {
        update_layer_surface.destroy();
    }
    if let Some(update_surface) = state.update_surface.take() {
        update_surface.destroy();
    }
    state.update_frame_callback = None;
}
