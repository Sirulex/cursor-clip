use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_client::protocol::wl_callback;
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::frontend::frontend_state::State;
use crate::frontend::dispatch::layer_shell::cleanup_update_layer;
use log::debug;

#[derive(Debug, Clone)]
pub enum FrameCallbackData {
    CaptureLayer,
    UpdateLayer,
    UpdateLayerFrameCount(u8), // Track frame count for minimal delay
}

impl Dispatch<wl_callback::WlCallback, FrameCallbackData> for State {
    fn event(
        state: &mut State,
        _callback: &wl_callback::WlCallback,
        event: wl_callback::Event,
        data: &FrameCallbackData,
        _conn: &Connection,
        qhandle: &QueueHandle<State>,
    ) {
        if let wl_callback::Event::Done { callback_data: _ } = event {
            match data {
                FrameCallbackData::CaptureLayer => {
                    debug!("Capture layer frame callback received - creating update surface");
                    state.capture_frame_callback = None;
                    setup_update_layer(state, qhandle);
                }
                FrameCallbackData::UpdateLayer => {
                    debug!("Update layer frame callback received - starting frame counting");
                    state.update_frame_callback = None;
                    schedule_next_frame_check(state, qhandle, 0);
                }
                FrameCallbackData::UpdateLayerFrameCount(frame_count) => {
                    debug!("Update layer frame {} received", frame_count + 1);
                    if *frame_count < 2 {
                        schedule_next_frame_check(state, qhandle, frame_count + 1);
                    } else {
                        debug!("Minimal frames elapsed - cleaning up update layer");
                        cleanup_update_layer(state);
                    }
                }
            }
        }
    }
}

fn setup_update_layer(state: &mut State, qhandle: &QueueHandle<State>) {
    let Some(layer_shell) = &state.layer_shell else {
        eprintln!("Layer shell not available");
        return;
    };
    
    let Some(update_surface) = &state.update_surface else {
        eprintln!("Update surface not available");
        return;
    };

    // Create the update layer surface
    let update_layer_surface = layer_shell.get_layer_surface(
        update_surface,
        None,                                // output (None means all outputs)
        zwlr_layer_shell_v1::Layer::Overlay, // layer type
        "cursor-clip-update".to_string(),    // namespace
        qhandle,
        (), // user data
    );

    // Configure the update layer surface
    update_layer_surface.set_exclusive_zone(-1); // -1 -> don't reserve space
    update_layer_surface.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right
            | zwlr_layer_surface_v1::Anchor::Bottom,
    ); // Anchor to all edges

    // Store the layer surface in state
    state.update_layer_surface = Some(update_layer_surface);

    // Commit the update surface to trigger the configure event
    update_surface.commit();
}

fn schedule_next_frame_check(state: &mut State, qhandle: &QueueHandle<State>, frame_count: u8) {
    if let Some(update_surface) = &state.update_surface {
        debug!("Scheduling frame check #{}", frame_count + 1);
        let frame_callback = update_surface.frame(qhandle, FrameCallbackData::UpdateLayerFrameCount(frame_count));
        state.update_frame_callback = Some(frame_callback);
        
        // Commit to trigger the next frame
        update_surface.commit();
    } else {
        eprintln!("Update surface not available for frame check");
    }
}
