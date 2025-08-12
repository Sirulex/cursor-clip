use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::GlobalListContents,
    protocol::wl_registry,
};

use crate::state::State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut State,
        _proxy: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // React to dynamic global events if needed
        //println!("Received registry event: {:?}", _event);
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            println!("[{}] {} (v{})", name, interface, version);
        }
    }
}
