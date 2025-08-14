use std::sync::{Arc, Mutex};
use std::os::fd::AsFd;
use std::collections::HashMap;
use std::io::Read;
use wayland_client::protocol::{wl_seat, wl_display};
use wayland_client::{Connection, Dispatch, QueueHandle, Proxy};
use wayland_client::backend::ObjectId;
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_manager_v1::{self, ZwlrDataControlManagerV1}, 
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1}, 
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
};

use super::BackendState;
use crate::shared::WaylandConnectionManager;

pub struct WaylandClipboardMonitor {
    backend_state: Arc<Mutex<BackendState>>,
}

#[derive(Debug)]
struct ClipboardState {
    backend_state: Arc<Mutex<BackendState>>,
    data_control_manager: Option<ZwlrDataControlManagerV1>,
    data_control_device: Option<ZwlrDataControlDeviceV1>,
    seat: Option<wl_seat::WlSeat>,
    offers: HashMap<ObjectId, DataOffer>,
    current_selection: Option<DataOffer>,
}

#[derive(Debug, Clone)]
struct DataOffer {
    offer: ZwlrDataControlOfferV1,
    mime_types: Vec<String>,
}



impl ClipboardState {
    fn new(backend_state: Arc<Mutex<BackendState>>) -> Self {
        Self {
            backend_state,
            data_control_manager: None,
            data_control_device: None,
            seat: None,
            offers: HashMap::new(),
            current_selection: None,
        }
    }
}

impl WaylandClipboardMonitor {
    pub fn new(backend_state: Arc<Mutex<BackendState>>) -> Result<Self, String> {
        Ok(Self {
            backend_state,
        })
    }

    pub async fn start_monitoring(&mut self) -> Result<(), String> {
        println!("Starting clipboard monitor...");

        // Get shared connection and bound objects
        let shared_conn = WaylandConnectionManager::get_global()
            .ok_or("No shared Wayland connection available")?;
        
        let (manager, seat, mut event_queue) = {
            let mut manager_guard = shared_conn.lock().unwrap();
            
            // Create event queue from shared connection
            let event_queue = manager_guard.new_event_queue();
            let qh = event_queue.handle();
            
            // Bind backend protocols if not already bound
            manager_guard.bind_backend_protocols(&qh)
                .map_err(|e| format!("Failed to bind backend protocols: {}", e))?;
            
            let manager = manager_guard.data_control_manager.as_ref()
                .ok_or("Data control manager not bound")?
                .clone();
            let seat = manager_guard.seat.as_ref()
                .ok_or("Seat not bound")?
                .clone();
            
            (manager, seat, event_queue)
        };
        
        println!("Using shared Wayland connection and bound objects for clipboard monitoring");
        
        let mut state = ClipboardState::new(self.backend_state.clone());
        
        // Use the already bound objects instead of discovering them
        state.data_control_manager = Some(manager.clone());
        state.seat = Some(seat.clone());
        
        // Set up data control device using the shared bound objects
        println!("Setting up data control device...");
        let qh = event_queue.handle();
        let device = manager.get_data_device(&seat, &qh, ());
        state.data_control_device = Some(device);
        
        println!("✅ Successfully connected to Wayland data control interface");
        println!("🔍 Monitoring clipboard changes...\n");
        
        // Main event loop
        loop {
            event_queue.blocking_dispatch(&mut state)
                .map_err(|e| format!("Failed to dispatch events: {}", e))?;
        }
    }
}



impl Dispatch<ZwlrDataControlManagerV1, ()> for ClipboardState {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlManagerV1,
        _: zwlr_data_control_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<ClipboardState>,
    ) {
        // No events for the manager
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for ClipboardState {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<ClipboardState>,
    ) {
        // We don't need to handle seat events for this application
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for ClipboardState {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        conn: &Connection,
        _qh: &QueueHandle<ClipboardState>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                let object_id = id.id();

                println!("New data offer received with ID: {:?}", object_id);
                // The id is already bound to our event queue, we just need to store it
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
                        
                        // Only process if we haven't already processed this offer
                        if state.current_selection.as_ref().map(|s| s.offer.id()) != Some(object_id) {
                            state.current_selection = Some(data_offer.clone());
                            
                            // Use the new read_offer function similar to the example
                            read_offer(&data_offer.offer, &data_offer.mime_types, conn, state.backend_state.clone());
                        }
                    }
                } else {
                    println!("Selection cleared");
                    state.current_selection = None;
                }
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { .. } => {
                // We ignore primary selection as requested
            }
            _ => {}
        }
    }

    fn event_created_child(
        opcode: u16,
        qhandle: &QueueHandle<Self>,
    ) -> Arc<dyn wayland_client::backend::ObjectData> {
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

impl Dispatch<ZwlrDataControlOfferV1, ()> for ClipboardState {
    fn event(
        state: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<ClipboardState>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            let object_id = offer.id();
            if let Some(data_offer) = state.offers.get_mut(&object_id) {
                data_offer.mime_types.push(mime_type);
                //println!("Offer supports MIME type: {}", data_offer.mime_types.last().unwrap());
            }
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for ClipboardState {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlSourceV1,
        _: <ZwlrDataControlSourceV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<ClipboardState>,
    ) {
        // We don't use data control source in this app
    }
}

impl Dispatch<wl_display::WlDisplay, ()> for ClipboardState {
    fn event(
        _: &mut Self,
        _: &wl_display::WlDisplay,
        _: wl_display::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<ClipboardState>,
    ) {
        // Handle display events if needed
    }
}

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
fn read_offer(
    data_offer: &ZwlrDataControlOfferV1,
    mime_types: &[String],
    conn: &Connection,
    backend_state: Arc<Mutex<BackendState>>,
) {
    // Prioritize text/plain over other types, similar to the example
    for mime_type in mime_types {
        if mime_type == "text/plain" || mime_type == "text/plain;charset=utf-8" {
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
                        
                        // Add to backend state
                        let mut backend = backend_state.lock().unwrap();
                        backend.add_clipboard_item(content.trim().to_string());
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
