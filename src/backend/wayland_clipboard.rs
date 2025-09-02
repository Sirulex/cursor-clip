use std::sync::{Arc, Mutex};
use std::os::fd::AsFd;
use std::collections::HashMap;
use std::io::Read;
use wayland_client::protocol::{wl_seat, wl_display};
use wayland_client::protocol::wl_registry;
use wayland_client::globals::GlobalListContents;
use wayland_client::{Connection, Dispatch, QueueHandle, Proxy};
use wayland_client::backend::ObjectId;
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_manager_v1::{self, ZwlrDataControlManagerV1}, 
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1}, 
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
};

use super::BackendState;
use wayland_client::{globals::{GlobalList, registry_queue_init}, EventQueue};

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
        println!("Starting clipboard monitor with its own Wayland connection...");

        // Establish independent connection
        let connection = Connection::connect_to_env()
            .map_err(|e| format!("Failed to connect to Wayland: {}", e))?;
        let (globals, mut event_queue): (GlobalList, EventQueue<ClipboardState>) =
            registry_queue_init::<ClipboardState>(&connection)
                .map_err(|e| format!("Failed to init registry: {}", e))?;

        let mut state = ClipboardState::new(self.backend_state.clone());

        // Roundtrip once for globals
        event_queue.roundtrip(&mut state)
            .map_err(|e| format!("Initial roundtrip failed: {}", e))?;

        // Bind required globals: wl_seat (for device) and data control manager
        let qh = event_queue.handle();

        // Bind seat
        if let Ok(seat) = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=9, ()) {
            state.seat = Some(seat.clone());
        } else {
            return Err("wl_seat not available".into());
        }

        // Bind data control manager
        if let Ok(data_control_manager) = globals.bind::<ZwlrDataControlManagerV1, _, _>(&qh, 2..=2, ()) {
            state.data_control_manager = Some(data_control_manager.clone());
            // Create device now that we have seat
            if let Some(seat) = &state.seat {
                let device = data_control_manager.get_data_device(seat, &qh, ());
                state.data_control_device = Some(device);
            }
        } else {
            return Err("zwlr_data_control_manager_v1 not available".into());
        }

        println!("✅ Wayland clipboard monitor initialized (independent connection)");
        println!("🔍 Monitoring clipboard changes...\n");

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

// Required so registry_queue_init can populate globals (must use GlobalListContents user data)
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ClipboardState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<ClipboardState>,
    ) {
        // GlobalList handles population; nothing else to do.
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
