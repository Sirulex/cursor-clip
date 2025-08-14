use std::sync::{Arc, Mutex, OnceLock};
use wayland_client::{
    Connection, EventQueue,
    globals::{GlobalList, registry_queue_init},
    protocol::{wl_compositor, wl_seat, wl_shm},
};
use wayland_protocols_wlr::{
    layer_shell::v1::client::zwlr_layer_shell_v1,
    virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1,
    data_control::v1::client::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
};
use wayland_protocols::{
    wp::{
        single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1,
        viewporter::client::wp_viewporter,
    },
    xdg::shell::client::xdg_wm_base,
};

/// Shared Wayland connection and bound resource objects manager
/// This ensures all components use the same connection and bound objects
#[derive(Debug)]
pub struct WaylandConnectionManager {
    pub connection: Connection,
    pub globals: GlobalList,
    
    // Core protocols - bound once and shared
    pub compositor: Option<wl_compositor::WlCompositor>,
    pub shm: Option<wl_shm::WlShm>,
    pub seat: Option<wl_seat::WlSeat>,
    
    // Frontend protocols
    pub layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pub xdg_wm_base: Option<xdg_wm_base::XdgWmBase>,
    pub viewporter: Option<wp_viewporter::WpViewporter>,
    pub single_pixel_buffer_manager: Option<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1>,
    pub virtual_pointer_manager: Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1>,
    
    // Backend protocols
    pub data_control_manager: Option<ZwlrDataControlManagerV1>,
}

// Global instance - initialized once by backend, used by frontend
static WAYLAND_CONNECTION: OnceLock<Arc<Mutex<WaylandConnectionManager>>> = OnceLock::new();

impl WaylandConnectionManager {
    /// Initialize the shared Wayland connection and discover globals
    pub fn initialize() -> Result<Arc<Mutex<Self>>, Box<dyn std::error::Error>> {
        println!("Initializing shared Wayland connection manager...");
        
        // Connect to Wayland
        let connection = Connection::connect_to_env()
            .map_err(|e| format!("Failed to connect to Wayland: {}", e))?;
        
        // Get globals using a temporary state for discovery
        let (globals, mut queue): (GlobalList, EventQueue<crate::frontend::frontend_state::State>) = 
            registry_queue_init::<crate::frontend::frontend_state::State>(&connection)?;
        
        // Create temporary state for initial roundtrip
        let mut temp_state = crate::frontend::frontend_state::State::new();
        queue.roundtrip(&mut temp_state)?;
        
        let manager = WaylandConnectionManager {
            connection,
            globals,
            compositor: None,
            shm: None,
            seat: None,
            layer_shell: None,
            xdg_wm_base: None,
            viewporter: None,
            single_pixel_buffer_manager: None,
            virtual_pointer_manager: None,
            data_control_manager: None,
        };
        
        let manager_arc = Arc::new(Mutex::new(manager));
        
        // Store globally
        if WAYLAND_CONNECTION.set(manager_arc.clone()).is_err() {
            return Err("Wayland connection already initialized".into());
        }
        
        println!("✅ Shared Wayland connection initialized successfully");
        
        Ok(manager_arc)
    }
    
    /// Get the global shared Wayland connection instance
    pub fn get_global() -> Option<Arc<Mutex<Self>>> {
        WAYLAND_CONNECTION.get().cloned()
    }
    
    /// Bind required protocols for frontend with the given queue handle
    pub fn bind_frontend_protocols<T>(&mut self, queue_handle: &wayland_client::QueueHandle<T>) -> Result<(), Box<dyn std::error::Error>> 
    where
        T: wayland_client::Dispatch<wl_compositor::WlCompositor, ()> + 
            wayland_client::Dispatch<wl_shm::WlShm, ()> + 
            wayland_client::Dispatch<wl_seat::WlSeat, ()> +
            wayland_client::Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> +
            wayland_client::Dispatch<xdg_wm_base::XdgWmBase, ()> +
            wayland_client::Dispatch<wp_viewporter::WpViewporter, ()> +
            wayland_client::Dispatch<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1, ()> +
            wayland_client::Dispatch<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, ()> +
            'static,
    {
        if self.compositor.is_none() {
            if let Ok(compositor) = self.globals.bind::<wl_compositor::WlCompositor, _, _>(queue_handle, 4..=5, ()) {
                println!("  ✓ wl_compositor bound (shared)");
                self.compositor = Some(compositor);
            } else {
                return Err("wl_compositor not available".into());
            }
        }
        
        if self.shm.is_none() {
            if let Ok(shm) = self.globals.bind::<wl_shm::WlShm, _, _>(queue_handle, 1..=1, ()) {
                println!("  ✓ wl_shm bound (shared)");
                self.shm = Some(shm);
            } else {
                return Err("wl_shm not available".into());
            }
        }
        
        if self.seat.is_none() {
            if let Ok(seat) = self.globals.bind::<wl_seat::WlSeat, _, _>(queue_handle, 7..=9, ()) {
                println!("  ✓ wl_seat bound (shared)");
                self.seat = Some(seat);
            } else {
                return Err("wl_seat not available".into());
            }
        }
        
        if self.layer_shell.is_none() {
            if let Ok(layer_shell) = self.globals.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(queue_handle, 4..=4, ()) {
                println!("  ✓ zwlr_layer_shell_v1 bound (shared)");
                self.layer_shell = Some(layer_shell);
            } else {
                return Err("zwlr_layer_shell_v1 not available".into());
            }
        }
        
        if self.xdg_wm_base.is_none() {
            if let Ok(xdg_wm_base) = self.globals.bind::<xdg_wm_base::XdgWmBase, _, _>(queue_handle, 2..=6, ()) {
                println!("  ✓ xdg_wm_base bound (shared)");
                self.xdg_wm_base = Some(xdg_wm_base);
            }
        }
        
        if self.viewporter.is_none() {
            if let Ok(viewporter) = self.globals.bind::<wp_viewporter::WpViewporter, _, _>(queue_handle, 1..=1, ()) {
                println!("  ✓ wp_viewporter bound (shared)");
                self.viewporter = Some(viewporter);
            }
        }
        
        if self.single_pixel_buffer_manager.is_none() {
            if let Ok(single_pixel_buffer_manager) = self.globals.bind::<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1, _, _>(queue_handle, 1..=1, ()) {
                println!("  ✓ wp_single_pixel_buffer_manager_v1 bound (shared)");
                self.single_pixel_buffer_manager = Some(single_pixel_buffer_manager);
            }
        }
        
        if self.virtual_pointer_manager.is_none() {
            if let Ok(virtual_pointer_manager) = self.globals.bind::<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, _, _>(queue_handle, 2..=2, ()) {
                println!("  ✓ zwlr_virtual_pointer_manager_v1 bound (shared)");
                self.virtual_pointer_manager = Some(virtual_pointer_manager);
            }
        }
        
        Ok(())
    }
    
    /// Bind required protocols for backend clipboard monitoring
    pub fn bind_backend_protocols<T>(&mut self, queue_handle: &wayland_client::QueueHandle<T>) -> Result<(), Box<dyn std::error::Error>>
    where
        T: wayland_client::Dispatch<ZwlrDataControlManagerV1, ()> + 
            wayland_client::Dispatch<wl_seat::WlSeat, ()> +
            'static,
    {
        // Ensure seat is bound first (shared with frontend)
        if self.seat.is_none() {
            if let Ok(seat) = self.globals.bind::<wl_seat::WlSeat, _, _>(queue_handle, 7..=9, ()) {
                println!("  ✓ wl_seat bound (shared)");
                self.seat = Some(seat);
            } else {
                return Err("wl_seat not available".into());
            }
        }
        
        if self.data_control_manager.is_none() {
            if let Ok(data_control_manager) = self.globals.bind::<ZwlrDataControlManagerV1, _, _>(queue_handle, 2..=2, ()) {
                println!("  ✓ zwlr_data_control_manager_v1 bound (shared)");
                self.data_control_manager = Some(data_control_manager);
            } else {
                return Err("zwlr_data_control_manager_v1 not available - clipboard monitoring will not work".into());
            }
        }
        
        Ok(())
    }
    
    /// Create a new event queue for this connection
    pub fn new_event_queue<T>(&self) -> EventQueue<T> {
        self.connection.new_event_queue()
    }
    
    /// Get a reference to the connection
    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}
