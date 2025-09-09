use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_pointer, wl_seat, wl_surface,
};

use wayland_protocols_wlr::{
    layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
};

use wayland_protocols::{
    wp::{
        single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1,
        viewporter::client::wp_viewporter,
    },
};

use crate::shared::ClipboardItemPreview;

pub struct State {
    pub compositor: Option<wl_compositor::WlCompositor>,
    pub layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pub pointer: Option<wl_pointer::WlPointer>,
    pub seat: Option<wl_seat::WlSeat>,
    pub single_pixel_buffer_manager: Option<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1>,
    pub viewporter: Option<wp_viewporter::WpViewporter>,
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
    pub clipboard_history: Vec<ClipboardItemPreview>,
}

impl State {
    pub fn new() -> Self {
        Self {
            compositor: None,
            layer_shell: None,
            pointer: None,
            seat: None,
            single_pixel_buffer_manager: None,
            viewporter: None,
            coords_received: false,
            received_x: 0.0,
            received_y: 0.0,
            capture_layer_clicked: false,
            capture_layer_ready: false,
            capture_surface: None,
            transparent_buffer: None,
            capture_layer_surface: None,
            capture_frame_callback: None,
            update_surface: None,
            update_layer_surface: None,
            update_frame_callback: None,
            clipboard_history: Vec::new(),
        }
    }
}
