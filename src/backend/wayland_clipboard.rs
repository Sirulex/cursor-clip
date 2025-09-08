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

use super::backend_state::BackendState;
use indexmap::IndexMap;
use log::{info, debug, warn, error};

// Wrapper struct that holds the shared backend state for dispatch implementations
pub struct MutexBackendState {
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
        // Establish Wayland connection
        let connection = Connection::connect_to_env()
            .map_err(|e| format!("Failed to connect to Wayland: {}", e))?;
        let (globals, mut event_queue): (GlobalList, EventQueue<MutexBackendState>) =
            registry_queue_init::<MutexBackendState>(&connection)
                .map_err(|e| format!("Failed to init registry: {}", e))?;

        // Create wrapper for shared state
        let mut shared_state_wrapper = MutexBackendState { backend_state: self.backend_state.clone() };

        // Roundtrip once for globals
        //event_queue.roundtrip(&mut shared_state_wrapper)
        //    .map_err(|e| format!("Initial roundtrip failed: {}", e))?;

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

        info!("Wayland clipboard monitor initialized, monitoring changes...");

        loop {
            // Dispatch pending events, then block waiting for new ones
            event_queue.blocking_dispatch(&mut shared_state_wrapper)
                .map_err(|e| format!("Failed to dispatch events: {}", e))?;
        }
    }
}

// ================= Dispatch Implementations =================

impl Dispatch<ZwlrDataControlManagerV1, ()> for MutexBackendState {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlManagerV1,
        _: zwlr_data_control_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<MutexBackendState>,
    ) {
        // No events for the manager
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for MutexBackendState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<MutexBackendState>,
    ) {
        // GlobalList handles population; nothing else to do.
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for MutexBackendState {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<MutexBackendState>,
    ) {
        // We don't need to handle seat events for this application
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        conn: &Connection,
        _qh: &QueueHandle<MutexBackendState>,
    ) {
        let mut state = wrapper.backend_state.lock().unwrap();
        
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                let object_id = id.id();
                debug!("New data offer received with ID: {:?}", object_id);
                state.mime_type_offers.insert(object_id, Vec::new());
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                if let Some(offer_id) = id {
                    let offer_key = offer_id.id();
                    debug!("Selection changed to offer ID: {:?}", offer_key);

                    let already_current = state.current_data_offer.as_ref().map(|o| o == &offer_key).unwrap_or(false);
                    if let Some(mime_list) = state.mime_type_offers.get(&offer_key).cloned() {
                        debug!("New clipboard content available with {} MIME types", mime_list.len());
                        if state.suppress_next_selection_read {
                            state.current_data_offer = Some(offer_key.clone());
                            debug!("Suppressed reading our own just-set selection; waiting for Cancelled to re-enable reads");
                        } else if !already_current {
                            state.current_data_offer = Some(offer_key.clone());
                            read_all_data_formats(&offer_id, &mime_list, conn, &mut state);
                            //remove old offer entries and their corresponding MIME types as new ones will be generated for future selections
                            state.mime_type_offers.clear();
                        }
                    }
                } else {
                    debug!("Selection cleared");
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

impl Dispatch<ZwlrDataControlOfferV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<MutexBackendState>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            let object_id = offer.id();
            debug!("Offer event: MIME type offered: {}", mime_type.clone());
            let mut state = wrapper.backend_state.lock().unwrap();
            if let Some(mime_list) = state.mime_type_offers.get_mut(&object_id) { mime_list.push(mime_type); }
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for MutexBackendState {
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
                debug!("Data source Send event for MIME type: {}", mime_type);
                if let Some(item_id) = state.current_source_entry_id {
                    if let Some(item) = state.get_item_by_id(item_id) {
                        use std::io::Write;
                        let mut file: std::fs::File = fd.into();
                        if let Some(bytes) = item.mime_data.get(&mime_type) {
                            if let Err(e) = file.write_all(bytes) {
                                error!(
                                    "Failed writing selection data (id {}, mime {}): {e}",
                                    item_id, mime_type
                                );
                            } else {
                                debug!(
                                    "Wrote {} bytes for id {} (mime {})", bytes.len(), item_id, mime_type);
                            }
                        } else {
                            warn!("No data stored for MIME {} (id {}), nothing written", mime_type, item_id);
                        }
                    } else {
                        warn!("Clipboard item id {} no longer exists in history", item_id);
                    }
                } else {
                    warn!("No current_source_id set when Send event received");
                }
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                debug!("Data source cancelled. Last offered content (object id {:?})", event_source.id());
                //Re-enabled reading new selections if currently active selection is cancelled, therefore external client took over 
                //if the cancelled event is not for the currently active selection, it was our previous selection -> new entry chosen within clipboard manager
                if state.current_source_object.as_ref().map(|s| s.id()) == Some(event_source.id()) {
                    state.suppress_next_selection_read = false;
                    state.current_source_object = None;
                    debug!("Re-enabled selection reading (external client took over)");
                }
                event_source.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_display::WlDisplay, ()> for MutexBackendState {
    fn event(
        _: &mut Self,
        _: &wl_display::WlDisplay,
        _: wl_display::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<MutexBackendState>,
    ) {
        // Handle display events if needed
    }
}

// ================= Helper functions =================

/// Create a pipe for reading clipboard data, returning OwnedFd handles.
fn create_pipes() -> Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd), Box<dyn std::error::Error>> {
    use std::os::fd::FromRawFd;
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let reader = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[0]) };
    let writer = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[1]) };
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
        let (reader_fd, writer_fd) = match create_pipes() {
            Ok(pair) => pair,
            Err(err) => { warn!("Could not open pipe to read data for {}: {:?}", mime, err); continue; }
        };
        debug!("Requesting {} content...", mime);
        data_offer.receive(mime.clone(), writer_fd.as_fd());
        // Drop writer side so the provider gets EOF after writing
        drop(writer_fd);
        if let Err(e) = conn.flush() { warn!("Flush failed: {e}"); }
        // Convert OwnedFd to File for reading
        let mut reader_file = std::fs::File::from(reader_fd);
        let mut buf = Vec::new();
        match reader_file.read_to_end(&mut buf) {
            Ok(_) => {
                if !buf.is_empty() { mime_map.insert(mime, buf); }
            }
            Err(e) => warn!("Failed reading data for mime: {e}"),
        }
    }

    if !mime_map.is_empty() {
        if let Some(new_id) = backend_state.add_clipboard_item_from_mime_map(mime_map) {
            // Only take ownership if we're NOT in monitor-only mode
            if !backend_state.monitor_only && !backend_state.suppress_next_selection_read {
                if let Err(e) = backend_state.set_clipboard_by_id(new_id) {
                    warn!("Failed to take ownership of selection id {}: {}", new_id, e);
                } else {
                    debug!("Took ownership of external selection (id {})", new_id);
                }
            }
        }
    }
}


