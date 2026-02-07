use std::sync::{Arc, Mutex};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};
use std::sync::Arc as StdArc;

use crate::backend::backend_state::{BackendState, DataControlProtocol};
use crate::backend::ext_data_control;
use indexmap::IndexMap;
use bytes::Bytes;
use log::{info, debug, warn, error};

// Wrapper struct that holds the shared backend state for dispatch implementations
pub struct MutexBackendState {
    pub backend_state: Arc<Mutex<BackendState>>,
}

pub struct WaylandClipboardMonitor {
    backend_state: Arc<Mutex<BackendState>>,
}

impl WaylandClipboardMonitor {
    pub const fn new(backend_state: Arc<Mutex<BackendState>>) -> Self {
        Self { backend_state }
    }

    pub fn start_monitoring(&self) -> Result<(), String> {
        // Establish Wayland connection
        let connection = Connection::connect_to_env()
            .map_err(|e| format!("Failed to connect to Wayland: {e}"))?;
        let (globals, mut event_queue): (GlobalList, EventQueue<MutexBackendState>) =
            registry_queue_init::<MutexBackendState>(&connection)
                .map_err(|e| format!("Failed to init registry: {e}"))?;

        // Create wrapper for shared state
        let mut shared_state_wrapper = MutexBackendState { backend_state: self.backend_state.clone() };

        // Bind required globals
        let qh = event_queue.handle();
        // Store queue handle inside BackendState for direct selection setting
        {
            let mut state = self.backend_state.lock().unwrap();
            state.qh = Some(qh.clone());
            state.connection = Some(connection);
        }

        // Bind seat
        if let Ok(seat) = globals.bind::<wayland_client::protocol::wl_seat::WlSeat, _, _>(&qh, 1..=9, ()) {
            let mut state = self.backend_state.lock().unwrap();
            state.seat = Some(seat);
        } else {
            let msg = "Critical Wayland interface 'wl_seat' is not available. \
            Your current compositor/session did not expose an input seat, which is required to create a data device for clipboard access. \
            Clipboard monitoring cannot start, exiting.";
            error!("{msg}");
            std::process::exit(1);
        }

        // Try wlroots data control manager first
        let wlr_available = globals.bind::<ZwlrDataControlManagerV1, _, _>(&qh, 2..=2, ()).is_ok();

        // Try ext data control manager
        let ext_available = globals.bind::<ext_data_control::ExtDataControlManagerV1, _, _>(&qh, 1..=1, ()).is_ok();

        info!("Available data control protocols - wlroots: {}, ext: {}", wlr_available, ext_available);

        if wlr_available {
            // Use wlroots protocol
            self.bind_wlr_protocol(&globals, &qh)?;
        } else if ext_available {
            // Use ext (standard) protocol
            self.bind_ext_protocol(&globals, &qh)?;
        } else {
            let msg = "No supported data control protocol available. \
            Tried both 'zwlr_data_control_manager_v1' (wlroots) and 'ext_data_control_manager_v1' (standard). \
            Your compositor likely does not support clipboard access via these protocols. \
            Clipboard monitoring cannot function without it, exiting.";
            error!("{msg}");
            std::process::exit(1);
        }

        info!("Wayland clipboard monitor initialized, monitoring changes...");

        loop {
            // Dispatch pending events, then block waiting for new ones
            event_queue.blocking_dispatch(&mut shared_state_wrapper)
                .map_err(|e| format!("Failed to dispatch events: {e}"))?;
        }
    }

    fn bind_wlr_protocol(&self, globals: &GlobalList, qh: &QueueHandle<MutexBackendState>) -> Result<(), String> {
        let data_control_manager = globals
            .bind::<ZwlrDataControlManagerV1, _, _>(qh, 2..=2, ())
            .map_err(|_| "Failed to bind wlroots data control manager".to_string())?;

        let mut state = self.backend_state.lock().unwrap();
        state.active_protocol = Some(DataControlProtocol::Wlr);
        state.data_control_manager = Some(data_control_manager.clone());

        // Create device now that we have seat
        if let Some(seat) = &state.seat {
            let device = data_control_manager.get_data_device(seat, qh, ());
            state.data_control_device = Some(device);
        }

        info!("Using wlroots data control protocol (zwlr_data_control_manager_v1)");
        Ok(())
    }

    fn bind_ext_protocol(&self, globals: &GlobalList, qh: &QueueHandle<MutexBackendState>) -> Result<(), String> {
        let data_control_manager = globals
            .bind::<ext_data_control::ExtDataControlManagerV1, _, _>(qh, 1..=1, ())
            .map_err(|_| "Failed to bind ext data control manager".to_string())?;

        let mut state = self.backend_state.lock().unwrap();
        state.active_protocol = Some(DataControlProtocol::Ext);
        state.ext_data_control_manager = Some(data_control_manager.clone());

        // Create device now that we have seat
        if let Some(seat) = &state.seat {
            let device = data_control_manager.get_data_device(seat, qh, ());
            state.ext_data_control_device = Some(device);
        }

        info!("Using standard data control protocol (ext_data_control_manager_v1)");
        Ok(())
    }
}

impl Drop for WaylandClipboardMonitor {
    fn drop(&mut self) {
        if let Ok(mut state) = self.backend_state.lock() {
            // Clean up wlroots objects
            if let Some(dev) = state.data_control_device.take() {
                dev.destroy();
            }
            if let Some(src) = state.current_source_object.take() {
                src.destroy();
            }
            if let Some(mgr) = state.data_control_manager.take() {
                mgr.destroy();
            }

            // Clean up ext objects
            if let Some(dev) = state.ext_data_control_device.take() {
                dev.destroy();
            }
            if let Some(src) = state.ext_current_source_object.take() {
                src.destroy();
            }
            if let Some(mgr) = state.ext_data_control_manager.take() {
                mgr.destroy();
            }

            state.seat.take();
            if let Some(conn) = &state.connection {
                let _ = conn.flush();
            }
        }
    }
}

// ================= Dispatch Implementations for wlroots =================

impl Dispatch<ZwlrDataControlDeviceV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        (): &(),
        conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let mut state = wrapper.backend_state.lock().unwrap();

        // Only process if we're using wlroots protocol
        if state.active_protocol != Some(DataControlProtocol::Wlr) {
            return;
        }

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

                    let already_current = state.current_data_offer.as_ref().is_some_and(|o| o == &offer_key);
                    if let Some(mime_list) = state.mime_type_offers.get(&offer_key).cloned() {
                        debug!("New clipboard content available with {} MIME types", mime_list.len());
                        if state.suppress_next_selection_read {
                            state.current_data_offer = Some(offer_key);
                            debug!("Suppressed reading our own just-set selection; waiting for Cancelled to re-enable reads");
                            offer_id.destroy();
                        } else if !already_current {
                            state.current_data_offer = Some(offer_key);
                            process_all_data_formats_wlr(&offer_id, mime_list, conn, &mut state);
                            state.mime_type_offers.clear();
                            offer_id.destroy();
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
            _ => panic!("Unknown child object for opcode {opcode}"),
        }
    }
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            let object_id = offer.id();
            debug!("Offer event: MIME type offered: {mime_type}");
            let mut state = wrapper.backend_state.lock().unwrap();
            if let Some(mime_list) = state.mime_type_offers.get_mut(&object_id) {
                if !mime_type.starts_with("video") { mime_list.push(mime_type); }
            }
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        event_source: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut state = wrapper.backend_state.lock().unwrap();

        match event {
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                debug!("Data source Send event for MIME type: {mime_type}");
                if let Some(item_id) = state.current_source_entry_id {
                    if let Some(item) = state.get_item_by_id(item_id) {
                        use std::io::Write;
                        let mut file: std::fs::File = fd.into();
                        if let Some(bytes) = item.mime_data.get(&mime_type) {
                            if let Err(e) = file.write_all(bytes.as_ref()) {
                                error!(
                                    "Failed writing selection data (id {item_id}, mime {mime_type}): {e}",
                                );
                            } else {
                                debug!("Wrote {} bytes for id {item_id} (mime {mime_type})", bytes.len());
                            }
                        } else {
                            warn!("No data stored for MIME {mime_type} (id {item_id}), nothing written");
                        }
                    } else {
                        warn!("Clipboard item id {item_id} no longer exists in history");
                    }
                } else {
                    warn!("No current_source_id set when Send event received");
                }
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                debug!("Data source cancelled. Last offered content (object id {:?})", event_source.id());
                if state.current_source_object.as_ref().map(Proxy::id) == Some(event_source.id()) {
                    state.suppress_next_selection_read = false;
                    state.current_source_object = None;
                    debug!("Re-enabled selection reading (external client took over)");
                }
                drop(state);
                event_source.destroy();
            }
            _ => {}
        }
    }
}

// ================== No-op Dispatch Implementations =================

delegate_noop!(MutexBackendState: ignore ZwlrDataControlManagerV1);
delegate_noop!(MutexBackendState: ignore WlSeat);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for MutexBackendState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<MutexBackendState>,
    ) {
        // No-op
    }
}

// ================= Helper functions =================

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

fn process_all_data_formats_wlr(
    data_offer: &ZwlrDataControlOfferV1,
    mime_types: Vec<String>,
    conn: &Connection,
    backend_state: &mut BackendState,
) {
    use std::os::fd::AsFd;
    use std::io::Read;

    if mime_types.is_empty() { return; }

    let mut mime_map: IndexMap<String, Bytes> = IndexMap::new();

    for mime in mime_types {
        let (reader_fd, writer_fd) = match create_pipes() {
            Ok(pair) => pair,
            Err(err) => { warn!("Could not open pipe to read data for {mime}: {err:?}"); continue; }
        };
        debug!("Requesting {mime} content...");
        data_offer.receive(mime.clone(), writer_fd.as_fd());
        drop(writer_fd);
        if let Err(e) = conn.flush() { warn!("Flush failed: {e}"); }

        let mut reader_file = std::fs::File::from(reader_fd);
        let mut buf = Vec::new();
        match reader_file.read_to_end(&mut buf) {
            Ok(_) => {
                if !buf.is_empty() { mime_map.insert(mime, Bytes::from(buf)); }
            }
            Err(e) => warn!("Failed reading data for mime: {e}"),
        }
    }

    if !mime_map.is_empty() {
        if let Some(new_id) = backend_state.add_clipboard_item_from_mime_map(mime_map) {
            if !backend_state.monitor_only && !backend_state.suppress_next_selection_read {
                if let Err(e) = backend_state.set_clipboard_by_id(new_id) {
                    warn!("Failed to take ownership of selection id {new_id}: {e}");
                } else {
                    debug!("Took ownership of external selection (id {new_id})");
                }
            }
        }
    }
}
