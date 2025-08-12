use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_client::protocol::wl_callback;
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use std::{thread, time::Duration};

use crate::state::State;
use crate::buffer;

#[derive(Debug, Clone)]
pub enum FrameCallbackData {
    CaptureLayer,
    UpdateLayer,
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
        match event {
            wl_callback::Event::Done { callback_data: _ } => {
                match data {
                    FrameCallbackData::CaptureLayer => {
                        println!("Capture layer frame callback received - creating update surface");
                        state.capture_frame_callback = None;
                        
                        // Create and configure the update layer surface
                        create_update_layer_surface(state, qhandle);
                    }
                    FrameCallbackData::UpdateLayer => {
                        println!("Update layer frame callback received - waiting 2 seconds before cleanup");
                        state.update_frame_callback = None;
                        
                        // Wait 2 seconds before cleaning up
                        thread::sleep(Duration::from_secs(1));
                        
                        // Close and cleanup update layer resources
                        cleanup_update_layer(state);
                    }
                }
            }
            _ => {}
        }
    }
}

fn create_update_layer_surface(state: &mut State, qhandle: &QueueHandle<State>) {
    let Some(layer_shell) = &state.layer_shell else {
        eprintln!("Layer shell not available");
        return;
    };
    
    let Some(update_surface) = &state.update_surface else {
        eprintln!("Update surface not available");
        return;
    };

    let Some(shm) = &state.shm else {
        eprintln!("SHM not available");
        return;
    };

    // Create a red buffer for the update surface
    if let Ok((_red_pool, red_buffer)) = buffer::create_red_buffer(shm, 1, 1, qhandle) {
        state.update_buffer = Some(red_buffer);
    } else {
        eprintln!("Failed to create red buffer for update surface");
        return;
    }

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

    update_layer_surface.set_margin(200, 200, 200, 200);

    // Store the layer surface in state
    state.update_layer_surface = Some(update_layer_surface);

    // Commit the update surface to trigger the configure event
    update_surface.commit();
}

fn cleanup_update_layer(state: &mut State) {
    println!("Cleaning up update layer resources");
    
    // Destroy the update layer surface if it exists
    if let Some(update_layer_surface) = state.update_layer_surface.take() {
        println!("Destroying update layer surface");
        update_layer_surface.destroy();
    }
    
    // Clean up the update surface
    if let Some(update_surface) = state.update_surface.take() {
        println!("Destroying update surface");
        update_surface.destroy();
    }
    
    // Clean up the update buffer
    if let Some(update_buffer) = state.update_buffer.take() {
        println!("Destroying update buffer");
        update_buffer.destroy();
    }
    
    // Clear the update frame callback reference (callbacks are auto-cleaned)
    if state.update_frame_callback.is_some() {
        println!("Clearing update frame callback reference");
        state.update_frame_callback = None;
    }
    
    println!("Update layer cleanup completed");
}
