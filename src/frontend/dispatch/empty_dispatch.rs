//! Centralized empty Wayland `Dispatch` implementations using `delegate_noop!`.
use crate::frontend::frontend_state::State;
use wayland_client::delegate_noop;

// Core protocol objects
use wayland_client::protocol::{
    wl_compositor::WlCompositor,
    wl_surface::WlSurface,
    wl_region::WlRegion,
    wl_buffer::WlBuffer,
    wl_registry::WlRegistry,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
};

// WLR layer shell
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

// Viewporter & single pixel buffer
use wayland_protocols::wp::viewporter::client::{
    wp_viewporter::WpViewporter,
    wp_viewport::WpViewport,
};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;

// Generate the noop dispatch implementations
delegate_noop!(State: WlCompositor);
delegate_noop!(State: WlRegion);   
delegate_noop!(State: WlSurface);
delegate_noop!(State: ZwlrLayerShellV1);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: WpSinglePixelBufferManagerV1);

//ignore Keyword as objects emit a Release event
//without ignore delegate_noop! macro would panic (unreachable) when an actual event arrives.
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlShm);

//-------------------------------------------------------------------------------
// Manual Dispatch implementations for specific interfaces needing custom logic
//-------------------------------------------------------------------------------

use wayland_client::{Connection, QueueHandle, Dispatch};
use wayland_client::globals::GlobalListContents;

// WlRegistry needs a custom Dispatch due to custom UserData (GlobalListContents)
impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut State,
        _registry: &WlRegistry,
        _event: wayland_client::protocol::wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<State>,
    ) {
        // No-op
    }
}

