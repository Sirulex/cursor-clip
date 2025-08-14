use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1,
};

use crate::frontend::frontend_state::State;

impl Dispatch<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(
        _state: &mut State,
        _manager: &zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
        _event: zwlr_virtual_pointer_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle virtual pointer manager events if needed
    }
}

impl Dispatch<zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1, ()> for State {
    fn event(
        _state: &mut State,
        _virtual_pointer: &zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
        _event: zwlr_virtual_pointer_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle virtual pointer events if needed
    }
}
