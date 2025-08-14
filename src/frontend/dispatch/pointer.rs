use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_client::protocol::{wl_pointer, wl_seat};

use crate::frontend::frontend_state::State;

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
                state.received_x = surface_x;
                state.received_y = surface_y;
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
                // Update stored coordinates on motion
                //state.received_x = surface_x;
                //state.received_y = surface_y;
            }
            wl_pointer::Event::Button {
                serial: _,
                time,
                button,
                state: button_state,
            } => {
                println!("Pointer button {:?} at time {}: {:?}", button, time, button_state);
                
                // Check for left mouse button click (button 272 is left click)
                if button == 272 {
                    if let WEnum::Value(wl_pointer::ButtonState::Pressed) = button_state {
                        println!("Left mouse button clicked on capture layer - requesting full close");
                        state.capture_layer_clicked = true; // This will trigger cleanup in main loop
                    }
                }
            }
            _ => {}
        }
    }
}
