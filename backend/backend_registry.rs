use wayland_client::{
    Connection, protocol::wl_registry,
};
use crate::backend::backend::BackendState;

impl wayland_client::Dispatch<wl_registry::WlRegistry, ()> for BackendState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: <wl_registry::WlRegistry as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &wayland_client::QueueHandle<Self>,
    ) {
        // Handle registry events for backend
    }
}
