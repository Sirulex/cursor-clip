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
use indexmap::IndexMap;

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
                state.mime_type_offers.insert(object_id, data_offer);
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                if let Some(offer_id) = id {
                    let object_id = offer_id.id();
                    println!("Selection changed to offer ID: {:?}", object_id);
                    if let Some(data_offer) = state.mime_type_offers.get(&object_id).cloned() {
                        println!("New clipboard content available with {} MIME types", data_offer.mime_types.len());
                        
                        if state.suppress_next_selection_read {
                            //We keep suppressing until the compositor sends a Cancelled event for our source
                            state.current_data_offer = Some(data_offer.clone());
                            println!("(Suppressed reading our own just-set selection; waiting for Cancelled to re-enable reads)");
                            return;
                        }

                        // Only process if we haven't already processed this offer
                        if state.current_data_offer.as_ref().map(|s| s.offer.id()) != Some(object_id) {
                            state.current_data_offer = Some(data_offer.clone());
                            
                            read_all_data_formats(&data_offer.offer, &data_offer.mime_types, conn, &mut *state);
                        }
                    }
                } else {
                    println!("Selection cleared");
                    state.current_data_offer = None;
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
            println!("Offer event: MIME type offered: {}", mime_type.clone());
            let mut state = wrapper.backend_state.lock().unwrap();
            if let Some(data_offer) = state.mime_type_offers.get_mut(&object_id) {
                data_offer.mime_types.push(mime_type);
            }
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for SharedBackendStateWrapper {
    fn event(
        wrapper: &mut Self,
        event_source: &ZwlrDataControlSourceV1,
        event: <ZwlrDataControlSourceV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut state = wrapper.backend_state.lock().unwrap();
        
        match event {
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                println!("Data source Send event for MIME type: {}", mime_type);
                if let Some(item_id) = state.current_source_entry_id {
                    if let Some(item) = state.get_item_by_id(item_id) {
                        use std::os::unix::io::{IntoRawFd, FromRawFd};
                        let raw_fd = fd.into_raw_fd();
                        let mut fd_endpoint = unsafe { std::fs::File::from_raw_fd(raw_fd) };
                        if let Some(bytes) = item.mime_data.get(&mime_type) {
                            if let Err(e) = std::io::Write::write_all(&mut fd_endpoint, bytes) {
                                eprintln!("Failed writing selection data (id {}, mime {}): {e}", item_id, mime_type);
                            } else {
                                println!("✅ Wrote {} bytes for id {} (mime {})", bytes.len(), item_id, mime_type);
                            }
                        } else {
                            println!("⚠️ No data stored for MIME {} (id {}), nothing written", mime_type, item_id);
                        }
                    } else {
                        eprintln!("Clipboard item id {} no longer exists in history", item_id);
                    }
                } else {
                    eprintln!("No current_source_id set when Send event received");
                }
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                println!("🛑 Data source cancelled. Last offered content (id {})", event_source.id());
                //Re-enabled reading new selections if currently active selection is cancelled, therefore external client took over 
                //if the cancelled event is not for the currently active selection, it was our previous selection -> new entry chosen within clipboard manager
                if state.current_source_object.as_ref().map(|s| s.id()) == Some(event_source.id()) {
                    state.suppress_next_selection_read = false;
                    state.current_source_object = None;
                    println!("🔄 Re-enabled selection reading (external client took over)");
                }
                event_source.destroy();
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

fn read_all_data_formats(
    data_offer: &ZwlrDataControlOfferV1,
    mime_types: &[String],
    conn: &Connection,
    backend_state: &mut BackendState,
) {
    use std::os::fd::AsFd;
    use std::io::Read;

    let text_mimes: Vec<String> = mime_types.to_vec();
    if text_mimes.is_empty() { return; }

    let mut mime_map: IndexMap<String, Vec<u8>> = IndexMap::new();

    for mime in text_mimes {
        let (mut reader, writer) = match create_pipes() {
            Ok((reader, writer)) => (reader, writer),
            Err(err) => { eprintln!("Could not open pipe to read data for {}: {:?}", mime, err); continue; }
        };
        println!("Requesting {} content...", mime);
        data_offer.receive(mime.clone(), writer.as_fd());
        drop(writer);
        if let Err(e) = conn.flush() { eprintln!("Flush failed: {e}"); }
        let mut buf = Vec::new();
        match reader.read_to_end(&mut buf) {
            Ok(_) => {
                if !buf.is_empty() { mime_map.insert(mime, buf); }
            }
            Err(e) => eprintln!("Failed reading data for mime: {e}"),
        }
    }

    if !mime_map.is_empty() {
        backend_state.add_clipboard_item_from_mime_map(mime_map);
    }
}


