use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_pointer, wl_seat, wl_shm, wl_surface,
};

use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use wayland_protocols::wp::{
    single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1,
    viewporter::client::{wp_viewport, wp_viewporter},
};

#[derive(Default)]
pub struct State {
    pub compositor: Option<wl_compositor::WlCompositor>,
    pub layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pub pointer: Option<wl_pointer::WlPointer>,
    pub seat: Option<wl_seat::WlSeat>,
    pub single_pixel_buffer_manager:
        Option<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1>,
    pub viewporter: Option<wp_viewporter::WpViewporter>,
    pub shm: Option<wl_shm::WlShm>,
    pub coords_received: bool,
    pub received_x: f64,
    pub received_y: f64,
    pub capture_layer_clicked: bool,
    pub capture_layer_ready: bool,
    pub capture_surface: Option<wl_surface::WlSurface>,
    pub transparent_buffer: Option<wl_buffer::WlBuffer>,
    pub capture_layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    pub capture_frame_callback: Option<wl_callback::WlCallback>,
    pub update_surface: Option<wl_surface::WlSurface>,
    pub update_layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    pub update_frame_callback: Option<wl_callback::WlCallback>,
    pub capture_viewport: Option<wp_viewport::WpViewport>,
    pub update_viewport: Option<wp_viewport::WpViewport>,
    pub monitor_width: i32,
    pub monitor_height: i32,
}
