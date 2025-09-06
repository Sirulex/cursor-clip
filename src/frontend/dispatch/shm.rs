use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_client::protocol::{wl_buffer};

use crate::frontend::frontend_state::State;
use log::debug;



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
			debug!("Buffer released by compositor");
		}
	}
}
