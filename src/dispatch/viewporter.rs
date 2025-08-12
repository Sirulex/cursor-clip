use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1;

use crate::state::State;

impl Dispatch<wp_viewporter::WpViewporter, ()> for State {
    fn event(
        _state: &mut State,
        _viewporter: &wp_viewporter::WpViewporter,
        _event: wp_viewporter::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle viewporter events if needed
    }
}

impl Dispatch<wp_viewport::WpViewport, ()> for State {
    fn event(
        _state: &mut State,
        _viewport: &wp_viewport::WpViewport,
        _event: wp_viewport::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle viewport events if needed
    }
}

impl Dispatch<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1, ()> for State {
    fn event(
        _state: &mut State,
        _manager: &wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
        _event: wp_single_pixel_buffer_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<State>,
    ) {
        // Handle single pixel buffer manager events if needed
    }
}
