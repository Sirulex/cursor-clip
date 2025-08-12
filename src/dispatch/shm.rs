use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_client::protocol::{wl_buffer, wl_shm, wl_shm_pool};

use crate::state::State;

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(
        _state: &mut State,
        _shm: &wl_shm::WlShm,
        _event: wl_shm::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle shm events if needed
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    fn event(
        _state: &mut State,
        _pool: &wl_shm_pool::WlShmPool,
        _event: wl_shm_pool::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle shm pool events if needed
    }
}

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
            // Buffer is no longer used by the compositor
            println!("Buffer released by compositor");
        }
    }
}
