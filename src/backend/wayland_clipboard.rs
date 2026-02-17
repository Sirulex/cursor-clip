use crate::backend::backend_state::{BackendState, DataControlManager};
use std::sync::Arc as StdArc; // for event_created_child return type clarity
use std::sync::{Arc, Mutex};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
    ext_data_control_source_v1::{self, ExtDataControlSourceV1},
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};

use bytes::Bytes;
use indexmap::IndexMap;
use log::{debug, error, info, warn};

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
        let mut shared_state_wrapper = MutexBackendState {
            backend_state: self.backend_state.clone(),
        };

        // Bind required globals
        let qh = event_queue.handle();
        // Store queue handle inside BackendState for direct selection setting
        {
            let mut state = self.backend_state.lock().unwrap();
            state.qh = Some(qh.clone());
            state.connection = Some(connection);
        }

        // Bind seat
        if let Ok(seat) =
            globals.bind::<wayland_client::protocol::wl_seat::WlSeat, _, _>(&qh, 1..=9, ())
        {
            let mut state = self.backend_state.lock().unwrap();
            state.seat = Some(seat);
        } else {
            // `wl_seat` is a core Wayland interface needed to create a data device for clipboard monitoring.
            // Without it, we cannot function. Exit with a clear explanation.
            let msg = "Critical Wayland interface 'wl_seat' is not available. \
            Your current compositor/session did not expose an input seat, which is required to create a data device for clipboard access. \
            Clipboard monitoring cannot start, exiting.";
            error!("{msg}");
            std::process::exit(1);
        }

        // Bind data control manager: prefer standard ext-data-control, fallback to wlr-data-control.
        if let Ok(manager) = globals.bind::<ExtDataControlManagerV1, _, _>(&qh, 1..=1, ()) {
            let mut state = self.backend_state.lock().unwrap();
            state.data_control_manager = Some(DataControlManager::Ext(manager));
            if let (Some(data_control_manager), Some(seat)) =
                (&state.data_control_manager, &state.seat)
            {
                let device = data_control_manager.get_data_device(seat, &qh);
                state.data_control_device = Some(device);
            }
            info!("Using ext_data_control_manager_v1 clipboard protocol");
        } else if let Ok(manager) = globals.bind::<ZwlrDataControlManagerV1, _, _>(&qh, 2..=2, ()) {
            let mut state = self.backend_state.lock().unwrap();
            state.data_control_manager = Some(DataControlManager::Wlr(manager));
            if let (Some(data_control_manager), Some(seat)) =
                (&state.data_control_manager, &state.seat)
            {
                let device = data_control_manager.get_data_device(seat, &qh);
                state.data_control_device = Some(device);
            }
            info!("Using zwlr_data_control_manager_v1 clipboard protocol");
        } else {
            let msg = "Critical Wayland global objects for clipboard are unavailable. \
            Neither 'ext_data_control_manager_v1' nor 'zwlr_data_control_manager_v1' is exposed by this compositor/session. \
            Clipboard monitoring cannot function without one of these protocols, exiting.";
            error!("{msg}");
            std::process::exit(1);
        }

        info!("Wayland clipboard monitor initialized, monitoring changes...");

        loop {
            // Dispatch pending events, then block waiting for new ones
            event_queue
                .blocking_dispatch(&mut shared_state_wrapper)
                .map_err(|e| format!("Failed to dispatch events: {e}"))?;
        }
    }
}

impl Drop for WaylandClipboardMonitor {
    fn drop(&mut self) {
        if let Ok(mut state) = self.backend_state.lock() {
            if let Some(dev) = state.data_control_device.take() {
                dev.destroy();
            }
            if let Some(src) = state.current_source_object.take() {
                src.destroy();
            }
            if let Some(mgr) = state.data_control_manager.take() {
                mgr.destroy();
            }
            state.seat.take(); // wl_seat proxies auto-drop; no explicit destroy
            if let Some(conn) = &state.connection {
                let _ = conn.flush();
            }
        }
    }
}

// ================= Dispatch Implementations =================

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

        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                let object_id = id.id();
                debug!("New data offer received with ID: {object_id:?}");
                state.mime_type_offers.insert(object_id, Vec::new());
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                if let Some(offer_id) = id {
                    let offer_key = offer_id.id();
                    debug!("Selection changed to offer ID: {offer_key:?}");

                    let (mime_list, already_current, suppress_read) = {
                        let already_current = state
                            .current_data_offer
                            .as_ref()
                            .is_some_and(|o| o == &offer_key);
                        let mime_list = state.mime_type_offers.get(&offer_key).cloned();
                        (
                            mime_list,
                            already_current,
                            state.suppress_next_selection_read,
                        )
                    };

                    if let Some(mime_list) = mime_list {
                        debug!(
                            "New clipboard content available with {} MIME types",
                            mime_list.len()
                        );
                        if suppress_read {
                            state.current_data_offer = Some(offer_key);
                            debug!(
                                "Suppressed reading our own just-set selection; waiting for Cancelled to re-enable reads"
                            );
                            offer_id.destroy();
                            return;
                        }

                        if already_current {
                            offer_id.destroy();
                            return;
                        }

                        state.current_data_offer = Some(offer_key);
                        // Remove old offer entries as new ones will be generated for future selections
                        state.mime_type_offers.clear();
                        drop(state);

                        let mime_map = read_all_data_formats_wlr(&offer_id, mime_list, conn);
                        if !mime_map.is_empty() {
                            let mut state = wrapper.backend_state.lock().unwrap();
                            if let Some(new_id) = state.add_clipboard_item_from_mime_map(mime_map)
                                && !state.monitor_only
                                && !state.suppress_next_selection_read
                            {
                                if let Err(e) = state.set_clipboard_by_id(new_id) {
                                    warn!("Failed to take ownership of selection id {new_id}: {e}");
                                } else {
                                    debug!("Took ownership of external selection (id {new_id})");
                                }
                            }
                        }
                        offer_id.destroy();
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
            if let Some(mime_list) = state.mime_type_offers.get_mut(&object_id)
                && !mime_type.starts_with("video")
            {
                mime_list.push(mime_type);
            }
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        event_source: &ZwlrDataControlSourceV1,
        event: <ZwlrDataControlSourceV1 as wayland_client::Proxy>::Event,
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
                                debug!(
                                    "Wrote {} bytes for id {item_id} (mime {mime_type})",
                                    bytes.len()
                                );
                            }
                        } else {
                            warn!(
                                "No data stored for MIME {mime_type} (id {item_id}), nothing written"
                            );
                        }
                    } else {
                        warn!("Clipboard item id {item_id} no longer exists in history");
                    }
                } else {
                    warn!("No current_source_id set when Send event received");
                }
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                debug!(
                    "Data source cancelled. Last offered content (object id {:?})",
                    event_source.id()
                );
                //Re-enabled reading new selections if currently active selection is cancelled, therefore external client took over
                //if the cancelled event is not for the currently active selection, it was our previous selection -> new entry chosen within clipboard manager
                if state
                    .current_source_object
                    .as_ref()
                    .map(|source| source.id())
                    == Some(event_source.id())
                {
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

impl Dispatch<ExtDataControlDeviceV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        (): &(),
        conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let mut state = wrapper.backend_state.lock().unwrap();

        match event {
            ext_data_control_device_v1::Event::DataOffer { id } => {
                let object_id = id.id();
                debug!("New data offer received with ID: {object_id:?}");
                state.mime_type_offers.insert(object_id, Vec::new());
            }
            ext_data_control_device_v1::Event::Selection { id } => {
                if let Some(offer_id) = id {
                    let offer_key = offer_id.id();
                    debug!("Selection changed to offer ID: {offer_key:?}");

                    let (mime_list, already_current, suppress_read) = {
                        let already_current = state
                            .current_data_offer
                            .as_ref()
                            .is_some_and(|o| o == &offer_key);
                        let mime_list = state.mime_type_offers.get(&offer_key).cloned();
                        (
                            mime_list,
                            already_current,
                            state.suppress_next_selection_read,
                        )
                    };

                    if let Some(mime_list) = mime_list {
                        debug!(
                            "New clipboard content available with {} MIME types",
                            mime_list.len()
                        );
                        if suppress_read {
                            state.current_data_offer = Some(offer_key);
                            debug!(
                                "Suppressed reading our own just-set selection; waiting for Cancelled to re-enable reads"
                            );
                            offer_id.destroy();
                            return;
                        }

                        if already_current {
                            offer_id.destroy();
                            return;
                        }

                        state.current_data_offer = Some(offer_key);
                        state.mime_type_offers.clear();
                        drop(state);

                        let mime_map = read_all_data_formats_ext(&offer_id, mime_list, conn);
                        if !mime_map.is_empty() {
                            let mut state = wrapper.backend_state.lock().unwrap();
                            if let Some(new_id) = state.add_clipboard_item_from_mime_map(mime_map)
                                && !state.monitor_only
                                && !state.suppress_next_selection_read
                            {
                                if let Err(e) = state.set_clipboard_by_id(new_id) {
                                    warn!("Failed to take ownership of selection id {new_id}: {e}");
                                } else {
                                    debug!("Took ownership of external selection (id {new_id})");
                                }
                            }
                        }
                        offer_id.destroy();
                    }
                } else {
                    debug!("Selection cleared");
                    state.current_data_offer = None;
                }
            }
            ext_data_control_device_v1::Event::PrimarySelection { .. } => {
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
            0 => qhandle.make_data::<ExtDataControlOfferV1, ()>(()),
            _ => panic!("Unknown child object for opcode {opcode}"),
        }
    }
}

impl Dispatch<ExtDataControlOfferV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        offer: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            let object_id = offer.id();
            debug!("Offer event: MIME type offered: {mime_type}");
            let mut state = wrapper.backend_state.lock().unwrap();
            if let Some(mime_list) = state.mime_type_offers.get_mut(&object_id)
                && !mime_type.starts_with("video")
            {
                mime_list.push(mime_type);
            }
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        event_source: &ExtDataControlSourceV1,
        event: <ExtDataControlSourceV1 as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut state = wrapper.backend_state.lock().unwrap();

        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
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
                                debug!(
                                    "Wrote {} bytes for id {item_id} (mime {mime_type})",
                                    bytes.len()
                                );
                            }
                        } else {
                            warn!(
                                "No data stored for MIME {mime_type} (id {item_id}), nothing written"
                            );
                        }
                    } else {
                        warn!("Clipboard item id {item_id} no longer exists in history");
                    }
                } else {
                    warn!("No current_source_id set when Send event received");
                }
            }
            ext_data_control_source_v1::Event::Cancelled => {
                debug!(
                    "Data source cancelled. Last offered content (object id {:?})",
                    event_source.id()
                );
                if state
                    .current_source_object
                    .as_ref()
                    .map(|source| source.id())
                    == Some(event_source.id())
                {
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
delegate_noop!(MutexBackendState: ignore ExtDataControlManagerV1);
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

/// Create a pipe for reading clipboard data, returning `OwnedFd` handles.
fn create_pipes() -> Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd), Box<dyn std::error::Error>>
{
    use std::os::fd::FromRawFd;
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let reader = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[0]) };
    let writer = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[1]) };
    Ok((reader, writer))
}

/// Filters out mime types to speed up processing
fn select_target_mimes(available_mimes: &[String]) -> Vec<String> {
    let best_image_mime = available_mimes
        .iter()
        .find(|m| *m == "image/png")
        .or_else(|| available_mimes.iter().find(|m| *m == "image/jpeg"))
        .or_else(|| available_mimes.iter().find(|m| *m == "image/bmp"));

    if let Some(img) = best_image_mime {
        return vec![img.clone()];
    }

    available_mimes.to_vec()
}

fn read_all_data_formats_wlr(
    data_offer: &ZwlrDataControlOfferV1,
    mime_types: Vec<String>,
    conn: &Connection,
) -> IndexMap<String, Bytes> {
    use std::io::Read;
    use std::os::fd::AsFd;

    let mut mime_map: IndexMap<String, Bytes> = IndexMap::new();

    if mime_types.is_empty() {
        return mime_map;
    }

    let targets = select_target_mimes(&mime_types);

    for mime in targets {
        let (reader_fd, writer_fd) = match create_pipes() {
            Ok(pair) => pair,
            Err(err) => {
                warn!("Could not open pipe to read data for {mime}: {err:?}");
                continue;
            }
        };
        debug!("Requesting {mime} content...");
        data_offer.receive(mime.clone(), writer_fd.as_fd());
        // Drop writer side so the provider gets EOF after writing
        drop(writer_fd);
        if let Err(e) = conn.flush() {
            warn!("Flush failed: {e}");
        }
        // Convert OwnedFd to File for reading
        let mut reader_file = std::fs::File::from(reader_fd);
        let mut buf = Vec::new();
        match reader_file.read_to_end(&mut buf) {
            Ok(_) => {
                if !buf.is_empty() {
                    mime_map.insert(mime, Bytes::from(buf));
                }
            }
            Err(e) => warn!("Failed reading data for mime: {e}"),
        }
    }

    mime_map
}

fn read_all_data_formats_ext(
    data_offer: &ExtDataControlOfferV1,
    mime_types: Vec<String>,
    conn: &Connection,
) -> IndexMap<String, Bytes> {
    use std::io::Read;
    use std::os::fd::AsFd;

    let mut mime_map: IndexMap<String, Bytes> = IndexMap::new();

    if mime_types.is_empty() {
        return mime_map;
    }

    let targets = select_target_mimes(&mime_types);

    for mime in targets {
        let (reader_fd, writer_fd) = match create_pipes() {
            Ok(pair) => pair,
            Err(err) => {
                warn!("Could not open pipe to read data for {mime}: {err:?}");
                continue;
            }
        };
        debug!("Requesting {mime} content...");
        data_offer.receive(mime.clone(), writer_fd.as_fd());
        drop(writer_fd);
        if let Err(e) = conn.flush() {
            warn!("Flush failed: {e}");
        }
        let mut reader_file = std::fs::File::from(reader_fd);
        let mut buf = Vec::new();
        match reader_file.read_to_end(&mut buf) {
            Ok(_) => {
                if !buf.is_empty() {
                    mime_map.insert(mime, Bytes::from(buf));
                }
            }
            Err(e) => warn!("Failed reading data for mime: {e}"),
        }
    }

    mime_map
}
