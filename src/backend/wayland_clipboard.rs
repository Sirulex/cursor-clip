use std::sync::{Arc, Mutex};
use std::os::unix::io::{FromRawFd, RawFd, BorrowedFd};
use std::io::Read;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_manager_v1, zwlr_data_control_device_v1, zwlr_data_control_offer_v1,
};

use super::BackendState;

pub struct WaylandClipboardMonitor {
    backend_state: Arc<Mutex<BackendState>>,
}

pub struct ClipboardState {
    backend_state: Arc<Mutex<BackendState>>,
    data_control_manager: Option<zwlr_data_control_manager_v1::ZwlrDataControlManagerV1>,
    data_control_device: Option<zwlr_data_control_device_v1::ZwlrDataControlDeviceV1>,
    seat: Option<wl_seat::WlSeat>,
    current_offer: Option<zwlr_data_control_offer_v1::ZwlrDataControlOfferV1>,
    mime_types: Vec<String>,
}

impl WaylandClipboardMonitor {
    pub fn new(backend_state: Arc<Mutex<BackendState>>) -> Result<Self, String> {
        Ok(Self {
            backend_state,
        })
    }

    pub async fn start_monitoring(&mut self) -> Result<(), String> {
        println!("Starting Wayland clipboard monitoring...");
        
        let connection = Connection::connect_to_env()
            .map_err(|e| format!("Failed to connect to Wayland: {}", e))?;
        let display = connection.display();
        
        let mut event_queue = connection.new_event_queue();
        let qh = event_queue.handle();
        
        let mut state = ClipboardState {
            backend_state: self.backend_state.clone(),
            data_control_manager: None,
            data_control_device: None,
            seat: None,
            current_offer: None,
            mime_types: Vec::new(),
        };

        let _registry = display.get_registry(&qh, ());
        
        // Initial roundtrip to get globals
        event_queue.roundtrip(&mut state)
            .map_err(|e| format!("Failed to roundtrip: {}", e))?;

        loop {
            event_queue.blocking_dispatch(&mut state)
                .map_err(|e| format!("Failed to dispatch events: {}", e))?;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for ClipboardState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "zwlr_data_control_manager_v1" => {
                    println!("Found data control manager");
                    let manager = registry.bind::<zwlr_data_control_manager_v1::ZwlrDataControlManagerV1, _, _>(
                        name, version.min(2), qh, ()
                    );
                    state.data_control_manager = Some(manager);
                    
                    // Try to create device if we have a seat
                    if let Some(seat) = &state.seat {
                        if let Some(manager) = &state.data_control_manager {
                            let device = manager.get_data_device(seat, qh, ());
                            state.data_control_device = Some(device);
                            println!("Created data control device");
                        }
                    }
                }
                "wl_seat" => {
                    println!("Found seat");
                    let seat = registry.bind::<wl_seat::WlSeat, _, _>(
                        name, version.min(1), qh, ()
                    );
                    state.seat = Some(seat.clone());
                    
                    // Try to create device if we have a manager
                    if let Some(manager) = &state.data_control_manager {
                        let device = manager.get_data_device(&seat, qh, ());
                        state.data_control_device = Some(device);
                        println!("Created data control device");
                    }
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for ClipboardState {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Seat events are not needed for our clipboard monitoring
    }
}

impl Dispatch<zwlr_data_control_manager_v1::ZwlrDataControlManagerV1, ()> for ClipboardState {
    fn event(
        _: &mut Self,
        _: &zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
        _: zwlr_data_control_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // No events from the manager
    }
}

impl Dispatch<zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, ()> for ClipboardState {
    fn event(
        state: &mut Self,
        _: &zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                println!("New data offer received");
                state.current_offer = Some(id);
                state.mime_types.clear(); // Reset MIME types for new offer
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                println!("Clipboard selection changed");
                if let Some(_offer) = id {
                    // We'll handle the actual data reading when we get the MIME types
                    println!("Selection offer available");
                }
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { id } => {
                println!("Primary selection changed");
                if let Some(_offer) = id {
                    // We'll handle the actual data reading when we get the MIME types
                    println!("Primary selection offer available");
                }
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
                qhandle.make_data::<zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, ()>(())
            }
            _ => {
                panic!("Unknown child object for opcode {}", opcode);
            }
        }
    }
}

impl Dispatch<zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, ()> for ClipboardState {
    fn event(
        state: &mut Self,
        offer: &zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_offer_v1::Event::Offer { mime_type } => {
                println!("Data offer available for MIME type: {}", mime_type);
                state.mime_types.push(mime_type.clone());
                
                // If it's text/plain, request the data immediately
                if mime_type == "text/plain" {
                    println!("Requesting text/plain clipboard data");
                    
                    // Create a pipe to receive the data
                    match create_pipe() {
                        Ok((read_fd, write_fd)) => {
                            use std::os::unix::io::BorrowedFd;
                            let borrowed_fd = unsafe { BorrowedFd::borrow_raw(write_fd) };
                            offer.receive(mime_type, borrowed_fd);
                            close_fd(write_fd);
                            
                            // Read clipboard content in background
                            let backend_state = state.backend_state.clone();
                            tokio::spawn(async move {
                                read_clipboard_data(read_fd, backend_state).await;
                            });
                        }
                        Err(e) => {
                            eprintln!("Failed to create pipe: {}", e);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn create_pipe() -> Result<(RawFd, RawFd), Box<dyn std::error::Error>> {
    let mut fds = [0; 2];
    let result = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if result == 0 {
        Ok((fds[0], fds[1]))
    } else {
        Err("Failed to create pipe".into())
    }
}

fn close_fd(fd: RawFd) {
    unsafe {
        libc::close(fd);
    }
}

async fn read_clipboard_data(read_fd: RawFd, backend_state: Arc<Mutex<BackendState>>) {
    tokio::task::spawn_blocking(move || {
        let mut file = unsafe { std::fs::File::from_raw_fd(read_fd) };
        let mut buffer = Vec::new();
        
        if let Ok(_) = file.read_to_end(&mut buffer) {
            if let Ok(content) = String::from_utf8(buffer) {
                let content = content.trim().to_string();
                if !content.is_empty() {
                    println!("Received clipboard content: {}", content);
                    let mut backend = backend_state.lock().unwrap();
                    backend.add_clipboard_item(content);
                }
            }
        }
    }).await.ok();
}
