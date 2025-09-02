use std::sync::{Arc, Mutex};
use wayland_client::{Connection, EventQueue, Dispatch, QueueHandle, Proxy};
use wayland_client::globals::{GlobalList, registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_seat, wl_display, wl_registry};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_manager_v1::{self, ZwlrDataControlManagerV1},
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};
use std::sync::Arc as StdArc; // for event_created_child return type clarity

use super::backend_state::{BackendState, DataOffer};

// Wrapper struct that holds the shared backend state for dispatch implementations
pub struct SharedBackendStateWrapper {
    pub backend_state: Arc<Mutex<BackendState>>,
}

pub struct WaylandClipboardMonitor {
    backend_state: Arc<Mutex<BackendState>>,
}

impl WaylandClipboardMonitor {
    pub fn new(backend_state: Arc<Mutex<BackendState>>) -> Result<Self, String> {
        Ok(Self {
            backend_state,
        })
    }

    pub async fn start_monitoring(&mut self) -> Result<(), String> {
        println!("Starting unified Wayland clipboard monitor...");

        // Establish Wayland connection
    let connection = Connection::connect_to_env()
            .map_err(|e| format!("Failed to connect to Wayland: {}", e))?;
        let (globals, mut event_queue): (GlobalList, EventQueue<SharedBackendStateWrapper>) =
            registry_queue_init::<SharedBackendStateWrapper>(&connection)
                .map_err(|e| format!("Failed to init registry: {}", e))?;

        // Create wrapper for shared state
        let mut shared_state_wrapper = SharedBackendStateWrapper { backend_state: self.backend_state.clone() };

        // Roundtrip once for globals
        event_queue.roundtrip(&mut shared_state_wrapper)
            .map_err(|e| format!("Initial roundtrip failed: {}", e))?;

        // Bind required globals
        let qh = event_queue.handle();
        // Store queue handle inside BackendState for direct selection setting
        {
            let mut state = self.backend_state.lock().unwrap();
            state.qh = Some(qh.clone());
            state.connection = Some(connection.clone());
        }

        // Bind seat
        if let Ok(seat) = globals.bind::<wayland_client::protocol::wl_seat::WlSeat, _, _>(&qh, 1..=9, ()) {
            let mut state = self.backend_state.lock().unwrap();
            state.seat = Some(seat.clone());
        } else {
            return Err("wl_seat not available".into());
        }

        // Bind data control manager  
        if let Ok(data_control_manager) = globals.bind::<wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1, _, _>(&qh, 2..=2, ()) {
            let mut state = self.backend_state.lock().unwrap();
            state.data_control_manager = Some(data_control_manager.clone());
            
            // Create device now that we have seat
            if let Some(seat) = &state.seat {
                let device = data_control_manager.get_data_device(seat, &qh, ());
                state.data_control_device = Some(device);
            }
            
            // Store a compatible queue handle - we'll need to address this type mismatch
            // For now, we'll handle clipboard setting through a different mechanism
        } else {
            return Err("zwlr_data_control_manager_v1 not available".into());
        }

        println!("✅ Unified Wayland clipboard monitor initialized");
        println!("🔍 Monitoring clipboard changes...\n");

        loop {
            // First drain any already queued events (e.g., triggered after we set selection & flushed)
            if let Err(e) = event_queue.dispatch_pending(&mut shared_state_wrapper) { return Err(format!("Failed to dispatch pending events: {e}")); }
            // Then block waiting for new ones
            event_queue.blocking_dispatch(&mut shared_state_wrapper)
                .map_err(|e| format!("Failed to dispatch events: {}", e))?;
        }
    }
}

// ================= Dispatch Implementations =================

impl Dispatch<ZwlrDataControlManagerV1, ()> for SharedBackendStateWrapper {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlManagerV1,
        _: zwlr_data_control_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<SharedBackendStateWrapper>,
    ) {
        // No events for the manager
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for SharedBackendStateWrapper {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<SharedBackendStateWrapper>,
    ) {
        // GlobalList handles population; nothing else to do.
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for SharedBackendStateWrapper {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<SharedBackendStateWrapper>,
    ) {
        // We don't need to handle seat events for this application
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for SharedBackendStateWrapper {
    fn event(
        wrapper: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        conn: &Connection,
        _qh: &QueueHandle<SharedBackendStateWrapper>,
    ) {
        let mut state = wrapper.backend_state.lock().unwrap();
        
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                let object_id = id.id();
                println!("New data offer received with ID: {:?}", object_id);
                
                let data_offer = DataOffer {
                    offer: id,
                    mime_types: Vec::new(),
                };
                state.offers.insert(object_id, data_offer);
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                if let Some(offer_id) = id {
                    let object_id = offer_id.id();
                    println!("Selection changed to offer ID: {:?}", object_id);
                    if let Some(data_offer) = state.offers.get(&object_id).cloned() {
                        println!("New clipboard content available with {} MIME types", data_offer.mime_types.len());
                        
                        if state.suppress_next_selection_read {
                            // Do NOT reset the flag here anymore. We keep suppressing until the
                            // compositor sends a Cancelled event for our source. This avoids
                            // races where we might prematurely read our own offer and block.
                            state.current_selection = Some(data_offer.clone());
                            println!("(Suppressed reading our own just-set selection; waiting for Cancelled to re-enable reads)");
                            return;
                        }

                        // Only process if we haven't already processed this offer
                        if state.current_selection.as_ref().map(|s| s.offer.id()) != Some(object_id) {
                            state.current_selection = Some(data_offer.clone());
                            
                            // Read the clipboard content
                            read_clipboard_offer(&data_offer.offer, &data_offer.mime_types, conn, &mut *state);
                        }
                    }
                } else {
                    println!("Selection cleared");
                    state.current_selection = None;
                }
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { .. } => {
                // We ignore primary selection
            }
            _ => {}
        }
    }

    fn event_created_child(
        opcode: u16,
        qhandle: &QueueHandle<Self>,
    ) -> StdArc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            0 => {
                // DataOffer event - create a data offer object data
                qhandle.make_data::<ZwlrDataControlOfferV1, ()>(())
            }
            _ => {
                panic!("Unknown child object for opcode {}", opcode);
            }
        }
    }
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for SharedBackendStateWrapper {
    fn event(
        wrapper: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<SharedBackendStateWrapper>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            let object_id = offer.id();
            let mut state = wrapper.backend_state.lock().unwrap();
            if let Some(data_offer) = state.offers.get_mut(&object_id) {
                data_offer.mime_types.push(mime_type);
            }
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for SharedBackendStateWrapper {
    fn event(
        wrapper: &mut Self,
        source: &ZwlrDataControlSourceV1,
        event: <ZwlrDataControlSourceV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut state = wrapper.backend_state.lock().unwrap();
        
        match event {
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                println!("Data source Send event for MIME type: {}", mime_type);
                if mime_type == "text/plain" {
                    if let Some(item_id) = state.current_source_id {
                        if let Some(item) = state.get_item_by_id(item_id) {
                            use std::os::unix::io::{IntoRawFd, FromRawFd};
                            let raw_fd = fd.into_raw_fd();
                            let mut file = unsafe { std::fs::File::from_raw_fd(raw_fd) };
                            if let Err(e) = std::io::Write::write_all(&mut file, item.content.as_bytes()) {
                                eprintln!("Failed writing selection data (id {}): {e}", item_id);
                            } else {
                                println!("✅ Wrote clipboard data for id {} ({} bytes)", item_id, item.content.len());
                            }
                        } else {
                            eprintln!("Clipboard item id {} no longer exists in history", item_id);
                        }
                    } else {
                        eprintln!("No current_source_id set when Send event received");
                    }
                }
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                if let Some(item_id) = state.current_source_id {
                    if let Some(item) = state.get_item_by_id(item_id) {
                        println!("🛑 Data source cancelled. Last offered content (id {}): {}", item_id, item.content);
                    } else {
                        println!("🛑 Data source cancelled. (id {} not found in history)", item_id);
                    }
                } else {
                    println!("🛑 Data source cancelled. (No text stored)");
                }
                let cancelled_id = source.id();
                // If the cancelled source is our currently active one, clear it
                if state.current_source_object.as_ref().map(|s| s.id()) == Some(cancelled_id.clone()) {
                    state.current_source_object = None;
                    state.current_source_id = None;
                }
                // Only re-enable reading if this cancelled source matches the suppressed source
                if state.suppress_next_selection_read && state.suppressed_source_id == Some(cancelled_id.clone()) {
                    state.suppress_next_selection_read = false;
                    state.suppressed_source_id = None;
                    println!("🔄 Re-enabled selection reading after suppressed source cancellation");
                } else if state.suppressed_source_id != Some(cancelled_id.clone()) {
                    // Cancellation of an older source we replaced; keep suppression
                    println!("(Ignored cancellation of non-suppressed source {:?}; still suppressing)", cancelled_id);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_display::WlDisplay, ()> for SharedBackendStateWrapper {
    fn event(
        _: &mut Self,
        _: &wl_display::WlDisplay,
        _: wl_display::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<SharedBackendStateWrapper>,
    ) {
        // Handle display events if needed
    }
}

// ================= Helper functions =================

/// Create a pipe for reading clipboard data
fn create_pipes() -> Result<(std::fs::File, std::fs::File), Box<dyn std::error::Error>> {
    let mut fds = [0; 2];
    let result = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if result != 0 {
        return Err("Failed to create pipe".into());
    }
    
    // Convert file descriptors to Files for easier handling
    use std::os::fd::FromRawFd;
    let reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let writer = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    
    Ok((reader, writer))
}

/// Read data from a clipboard offer
fn read_clipboard_offer(
    data_offer: &ZwlrDataControlOfferV1,
    mime_types: &[String],
    conn: &Connection,
    backend_state: &mut BackendState,
) {
    use std::os::fd::AsFd;
    use std::io::Read;
    
    // Prioritize text/plain over other types
    for mime_type in mime_types {
        if mime_type == "text/plain" {
            let (mut reader, writer) = match create_pipes() {
                Ok((reader, writer)) => (reader, writer),
                Err(err) => {
                    eprintln!("Could not open pipe to read data: {:?}", err);
                    continue;
                }
            };
            
            println!("Requesting {} content...", mime_type);
            data_offer.receive(mime_type.clone(), writer.as_fd());
            drop(writer); // We won't write anything, the selection client will.
            
            // Flush to ensure data is sent
            conn.flush().expect("Failed to flush connection");
            
            // Read the data synchronously
            let mut content = String::new();
            match reader.read_to_string(&mut content) {
                Ok(_) => {
                    if !content.trim().is_empty() {
                        println!("📋 Clipboard content: {}", content.trim());
                        backend_state.add_clipboard_item(content.trim().to_string());
                    }
                }
                Err(err) => {
                    eprintln!("Failed to read clipboard content: {:?}", err);
                }
            }
            
            // Only read from the first suitable mime type
            break;
        }
    }
}


