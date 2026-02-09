use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_client::protocol::{wl_pointer, wl_seat};

use crate::frontend::frontend_state::State;
use crate::frontend::gtk_overlay;
use log::{debug};

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        debug!("Seat event: {event:?}");
        if let wl_seat::Event::Capabilities {
            capabilities: cap_event_enum,
        } = event
        {
            //detangle Capabilities enum
            if let WEnum::Value(capabilities) = cap_event_enum {
                debug!("Pointer capabilities detected");

                if capabilities.contains(wl_seat::Capability::Pointer) {
                    //no pattern matching as wl_seat::Capability is a bitfield
                    let pointer = seat.get_pointer(qhandle, ());
                    state.pointer = Some(pointer);
                    debug!("Pointer created");
                } else {
                    debug!("No pointer capabilities detected");
                }
            } else {
                debug!("Unknown capability enumerator");
            }
        }
        //impl release events todo
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        debug!("Pointer event received");
        match event {
            wl_pointer::Event::Enter {
                serial: _,
                surface,
                surface_x,
                surface_y,
            } => {
                debug!("Pointer entered surface: {surface:?} at ({surface_x}, {surface_y})");
                state.coords_received = true; // Set flag when coordinates are received
                state.received_x = surface_x;
                state.received_y = surface_y;
                state.pointer_x = surface_x;
                state.pointer_y = surface_y;
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                state.pointer_x = surface_x;
                state.pointer_y = surface_y;
            }
            wl_pointer::Event::Leave { serial: _, surface } => {
                debug!("Pointer left surface: {surface:?}");
            }
            wl_pointer::Event::Button {
                serial: _,
                time,
                button,
                state: button_state,
            } => {
                debug!("Pointer button {button:?} at time {time}: {button_state:?}");
                
                // Check for left mouse button click (button 272 = left click)
                if button == 272 {
                    if let WEnum::Value(wl_pointer::ButtonState::Pressed) = button_state {
                        if gtk_overlay::is_menu_open()
                            || gtk_overlay::is_point_inside_overlay(state.pointer_x, state.pointer_y)
                        {
                            return;
                        }
                        debug!("Left mouse button clicked on capture layer - requesting close");
                        state.capture_layer_clicked = true; // future handling of outside click to close overlay
                    }
                }
            }
            _ => {}
        }
    }
}
