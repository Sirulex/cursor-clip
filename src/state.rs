use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_pointer, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};

use wayland_protocols_wlr::{
    layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
    virtual_pointer::v1::client::{zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1},
};

use wayland_protocols::{
    wp::{
        single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1,
        viewporter::client::wp_viewporter,
    },
    xdg::shell::client::xdg_wm_base,
};

pub struct State {
    pub compositor: Option<wl_compositor::WlCompositor>,
    pub layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pub xdg_wm_base: Option<xdg_wm_base::XdgWmBase>,
    pub pointer: Option<wl_pointer::WlPointer>,
    pub shm: Option<wl_shm::WlShm>,
    pub pool: Option<wl_shm_pool::WlShmPool>,
    pub seat: Option<wl_seat::WlSeat>,
    pub single_pixel_buffer_manager:
        Option<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1>,
    pub viewporter: Option<wp_viewporter::WpViewporter>,
    pub virtual_pointer_manager: Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1>,
    pub virtual_pointer: Option<zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1>,
    pub coords_received: bool,
    pub capture_layer_ready: bool,
    pub capture_surface: Option<wl_surface::WlSurface>,
    pub capture_buffer: Option<wl_buffer::WlBuffer>,
    pub capture_layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    pub capture_frame_callback: Option<wl_callback::WlCallback>,
    pub update_surface: Option<wl_surface::WlSurface>,
    pub update_buffer: Option<wl_buffer::WlBuffer>,
    pub update_layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    pub update_frame_callback: Option<wl_callback::WlCallback>,
}

impl State {
    pub fn new() -> Self {
        Self {
            compositor: None,
            layer_shell: None,
            xdg_wm_base: None,
            pointer: None,
            shm: None,
            seat: None,
            pool: None,
            single_pixel_buffer_manager: None,
            viewporter: None,
            virtual_pointer_manager: None,
            virtual_pointer: None,
            coords_received: false,
            capture_layer_ready: false,
            capture_surface: None,
            capture_buffer: None,
            capture_layer_surface: None,
            capture_frame_callback: None,
            update_surface: None,
            update_buffer: None,
            update_layer_surface: None,
            update_frame_callback: None,
        }
    }
}
