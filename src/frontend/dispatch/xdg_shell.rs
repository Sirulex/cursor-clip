use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols::wp::viewporter::client::wp_viewport;

use crate::frontend::frontend_state::State;

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
                states: _,
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
                let viewport: wp_viewport::WpViewport =
                    viewporter.get_viewport(&update_surface, qhandle, ());

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
