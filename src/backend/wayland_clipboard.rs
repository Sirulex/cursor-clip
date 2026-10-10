use crate::backend::backend_state::{BackendState, DataControlManager};
use std::sync::Arc as StdArc; // for event_created_child return type clarity
use std::sync::atomic::{AtomicUsize, Ordering};
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

use crate::shared::PASSWORD_HINT_MIME;
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
        let connection = Connection::connect_to_env()
            .map_err(|e| format!("Failed to connect to Wayland: {e}"))?;
        let (globals, mut event_queue): (GlobalList, EventQueue<MutexBackendState>) =
            registry_queue_init::<MutexBackendState>(&connection)
                .map_err(|e| format!("Failed to init registry: {e}"))?;

        let mut shared_state_wrapper = MutexBackendState {
            backend_state: self.backend_state.clone(),
        };

        let qh = event_queue.handle();
        {
            let mut state = self.backend_state.lock().unwrap();
            state.qh = Some(qh.clone());
            state.connection = Some(connection);
        }

        // Bind wl_seat — required to create a data device for clipboard access.
        match globals.bind::<wayland_client::protocol::wl_seat::WlSeat, _, _>(&qh, 1..=9, ()) {
            Ok(seat) => self.backend_state.lock().unwrap().seat = Some(seat),
            Err(_) => {
                error!(
                    "Critical Wayland interface 'wl_seat' is not available. \
                    Your compositor did not expose an input seat, which is required \
                    for clipboard access. Exiting."
                );
                std::process::exit(1);
            }
        }

        // Bind data control manager: prefer ext-data-control, fall back to wlr-data-control.
        if let Ok(manager) = globals.bind::<ExtDataControlManagerV1, _, _>(&qh, 1..=1, ()) {
            self.bind_data_device(DataControlManager::Ext(manager), &qh);
            info!("Using ext_data_control_manager_v1 clipboard protocol");
        } else if let Ok(manager) = globals.bind::<ZwlrDataControlManagerV1, _, _>(&qh, 2..=2, ()) {
            self.bind_data_device(DataControlManager::Wlr(manager), &qh);
            info!("Using zwlr_data_control_manager_v1 clipboard protocol");
        } else {
            error!(
                "Neither 'ext_data_control_manager_v1' nor 'zwlr_data_control_manager_v1' is \
                available. Clipboard monitoring cannot function without one of these protocols. Exiting."
            );
            std::process::exit(1);
        }

        info!("Wayland clipboard monitor initialized, monitoring changes...");

        loop {
            event_queue
                .blocking_dispatch(&mut shared_state_wrapper)
                .map_err(|e| format!("Failed to dispatch events: {e}"))?;
        }
    }

    fn bind_data_device(&self, manager: DataControlManager, qh: &QueueHandle<MutexBackendState>) {
        let mut state = self.backend_state.lock().unwrap();
        // seat is guaranteed set before this is called
        let device = manager.get_data_device(state.seat.as_ref().unwrap(), qh);
        state.data_control_manager = Some(manager);
        state.data_control_device = Some(device);
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

// ================= Shared event helpers =================

/// Handle a new data offer from either Wlr or Ext device.
fn handle_data_offer(state: &mut BackendState, offer_id: wayland_client::backend::ObjectId) {
    debug!("New data offer received with ID: {offer_id:?}");
    state.mime_type_offers.insert(offer_id, Vec::new());
}

/// Register an offered MIME type for the given offer object, filtering out video types.
fn handle_offer_mime(
    state: &mut BackendState,
    offer_id: wayland_client::backend::ObjectId,
    mime_type: String,
) {
    debug!("Offer event: MIME type offered: {mime_type}");
    if let Some(mime_list) = state.mime_type_offers.get_mut(&offer_id)
        && !mime_type.starts_with("video")
    {
        mime_list.push(mime_type);
    }
}

/// Handle a Selection event from either Wlr or Ext device.
/// `read_mime_data` is called (with the lock released) to read the data from the offer.
fn handle_selection_event<F>(
    wrapper: &mut MutexBackendState,
    offer_id: wayland_client::backend::ObjectId,
    destroy_offer: impl FnOnce(),
    read_mime_data: F,
) where
    F: FnOnce(Vec<String>) -> IndexMap<String, Bytes>,
{
    let (mime_list, already_current, suppress_read) = {
        let state = wrapper.backend_state.lock().unwrap();
        let already_current = state
            .current_data_offer
            .as_ref()
            .is_some_and(|o| o == &offer_id);
        let mime_list = state.mime_type_offers.get(&offer_id).cloned();
        (
            mime_list,
            already_current,
            state.suppress_next_selection_read,
        )
    };

    let Some(mime_list) = mime_list else {
        return;
    };

    debug!(
        "New clipboard content available with {} MIME types",
        mime_list.len()
    );

    if suppress_read {
        wrapper.backend_state.lock().unwrap().current_data_offer = Some(offer_id);
        debug!(
            "Suppressed reading our own just-set selection; waiting for Cancelled to re-enable reads"
        );
        destroy_offer();
        return;
    }

    if already_current {
        destroy_offer();
        return;
    }

    {
        let mut state = wrapper.backend_state.lock().unwrap();
        state.current_data_offer = Some(offer_id);
        state.mime_type_offers.clear();
    }

    // Presence is authoritative even when the hint payload is empty/unreadable.
    let password_hint = mime_list.iter().any(|mime| mime == PASSWORD_HINT_MIME);
    let mut mime_map = read_mime_data(mime_list);
    if password_hint && !mime_map.is_empty() {
        mime_map
            .entry(PASSWORD_HINT_MIME.to_string())
            .or_insert_with(|| Bytes::from_static(b"secret"));
    }
    if !mime_map.is_empty() {
        let mut state = wrapper.backend_state.lock().unwrap();
        if let Some(new_id) = state.add_clipboard_item_from_mime_map(mime_map)
            // Let the password manager retain ownership and clear its own selection.
            && !password_hint
            && !state.monitor_only
            && !state.suppress_next_selection_read
        {
            if let Err(e) = state.set_clipboard_by_id(new_id, false) {
                warn!("Failed to take ownership of selection id {new_id}: {e}");
            } else {
                debug!("Took ownership of external selection (id {new_id})");
            }
        }
    }
    destroy_offer();
}

/// Transfers to paste targets that are still being written.
static ACTIVE_TRANSFERS: AtomicUsize = AtomicUsize::new(0);
const MAX_ACTIVE_TRANSFERS: usize = 8;
/// Give up on a receiver that stops reading for this long.
const TRANSFER_STALL_TIMEOUT_MS: i32 = 5_000;

/// Handle a Source Send event for either Wlr or Ext source.
fn handle_source_send(state: &BackendState, mime_type: String, fd: std::os::fd::OwnedFd) {
    debug!("Data source Send event for MIME type: {mime_type}");
    let Some(item_id) = state.current_source_entry_id else {
        warn!("No current_source_id set when Send event received");
        return;
    };
    // Opened under the lock so a concurrent delete or store switch cannot
    // remove the blob before the worker reads it.
    let blob = match state.open_item_payload(item_id, &mime_type) {
        Ok(blob) => blob,
        Err(e) => {
            warn!("{e}; nothing written");
            return;
        }
    };

    if ACTIVE_TRANSFERS.fetch_add(1, Ordering::SeqCst) >= MAX_ACTIVE_TRANSFERS {
        ACTIVE_TRANSFERS.fetch_sub(1, Ordering::SeqCst);
        warn!("Too many pending clipboard transfers; dropping request for {mime_type}");
        return;
    }

    // Reading, decrypting and writing happen off the Wayland thread and
    // without the backend lock, so a large payload or slow reader blocks nothing.
    let spawned = std::thread::Builder::new()
        .name("clipboard-send".into())
        .spawn(move || {
            let result = blob
                .read()
                .and_then(|data| write_with_stall_timeout(fd, &data).map(|()| data.len()));
            match result {
                Ok(len) => debug!("Wrote {len} bytes for id {item_id} (mime {mime_type})"),
                Err(e) => {
                    error!("Failed writing selection data (id {item_id}, mime {mime_type}): {e}");
                }
            }
            ACTIVE_TRANSFERS.fetch_sub(1, Ordering::SeqCst);
        });
    if let Err(e) = spawned {
        ACTIVE_TRANSFERS.fetch_sub(1, Ordering::SeqCst);
        error!("Could not start clipboard transfer thread: {e}");
    }
}

fn write_with_stall_timeout(fd: std::os::fd::OwnedFd, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::fd::AsRawFd;

    let raw_fd = fd.as_raw_fd();
    // SAFETY: raw_fd is a valid open descriptor owned by `fd` for this scope.
    unsafe {
        let flags = libc::fcntl(raw_fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(raw_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }

    let mut file: std::fs::File = fd.into();
    let mut remaining = data;
    while !remaining.is_empty() {
        match file.write(remaining) {
            Ok(0) => return Err("receiver closed the pipe".into()),
            Ok(written) => remaining = &remaining[written..],
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let mut poll_fd = libc::pollfd {
                    fd: raw_fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                // SAFETY: poll_fd is a single initialized pollfd that outlives the call.
                let ready = unsafe { libc::poll(&mut poll_fd, 1, TRANSFER_STALL_TIMEOUT_MS) };
                if ready == 0 {
                    return Err("receiver stopped reading".into());
                }
                if ready < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::Interrupted {
                        return Err(error.to_string());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

/// Handle a Source Cancelled event. Re-enables selection reading if this is the active source.
fn handle_source_cancelled(state: &mut BackendState, source_id: wayland_client::backend::ObjectId) {
    debug!("Data source cancelled (object id {source_id:?})");
    // If the cancelled source is still the active one, an external client took ownership — re-enable reads.
    if state.current_source_object.as_ref().map(|s| s.id()) == Some(source_id) {
        state.suppress_next_selection_read = false;
        state.current_source_object = None;
        state.current_source_entry_id = None;
        debug!("Re-enabled selection reading (external client took over)");
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
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                handle_data_offer(&mut wrapper.backend_state.lock().unwrap(), id.id());
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                if let Some(offer_id) = id {
                    let offer_key = offer_id.id();
                    debug!("Selection changed to offer ID: {offer_key:?}");
                    let conn = conn.clone();
                    handle_selection_event(
                        wrapper,
                        offer_key,
                        || offer_id.destroy(),
                        |mime_list| read_all_data_formats(&offer_id, mime_list, &conn),
                    );
                } else {
                    debug!("Selection cleared");
                    wrapper.backend_state.lock().unwrap().current_data_offer = None;
                }
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { .. } => {}
            _ => {}
        }
    }

    fn event_created_child(
        opcode: u16,
        qhandle: &QueueHandle<Self>,
    ) -> StdArc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            0 => qhandle.make_data::<ZwlrDataControlOfferV1, ()>(()),
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
            handle_offer_mime(
                &mut wrapper.backend_state.lock().unwrap(),
                offer.id(),
                mime_type,
            );
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
        match event {
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                handle_source_send(&wrapper.backend_state.lock().unwrap(), mime_type, fd);
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                let source_id = event_source.id();
                handle_source_cancelled(&mut wrapper.backend_state.lock().unwrap(), source_id);
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
        match event {
            ext_data_control_device_v1::Event::DataOffer { id } => {
                handle_data_offer(&mut wrapper.backend_state.lock().unwrap(), id.id());
            }
            ext_data_control_device_v1::Event::Selection { id } => {
                if let Some(offer_id) = id {
                    let offer_key = offer_id.id();
                    debug!("Selection changed to offer ID: {offer_key:?}");
                    let conn = conn.clone();
                    handle_selection_event(
                        wrapper,
                        offer_key,
                        || offer_id.destroy(),
                        |mime_list| read_all_data_formats(&offer_id, mime_list, &conn),
                    );
                } else {
                    debug!("Selection cleared");
                    wrapper.backend_state.lock().unwrap().current_data_offer = None;
                }
            }
            ext_data_control_device_v1::Event::PrimarySelection { .. } => {}
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
            handle_offer_mime(
                &mut wrapper.backend_state.lock().unwrap(),
                offer.id(),
                mime_type,
            );
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
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                handle_source_send(&wrapper.backend_state.lock().unwrap(), mime_type, fd);
            }
            ext_data_control_source_v1::Event::Cancelled => {
                let source_id = event_source.id();
                handle_source_cancelled(&mut wrapper.backend_state.lock().unwrap(), source_id);
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

/// Create a pipe, returning `OwnedFd` handles for the read and write ends.
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

/// Select the MIME types to actually read from the available list.
/// When image types are present, keep only PNG image data and all non-image variants.
fn select_target_mimes(available_mimes: &[String]) -> Vec<String> {
    if available_mimes.iter().any(|m| m.starts_with("image/")) {
        return available_mimes
            .iter()
            .filter(|m| m.as_str() == "image/png" || !m.starts_with("image/"))
            .cloned()
            .collect();
    }

    available_mimes.to_vec()
}

/// Trait abstracting over offer types that can receive data via a pipe fd.
trait DataOfferReceive {
    fn receive_mime(&self, mime_type: String, fd: std::os::fd::BorrowedFd<'_>);
}

impl DataOfferReceive for ZwlrDataControlOfferV1 {
    fn receive_mime(&self, mime_type: String, fd: std::os::fd::BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

impl DataOfferReceive for ExtDataControlOfferV1 {
    fn receive_mime(&self, mime_type: String, fd: std::os::fd::BorrowedFd<'_>) {
        self.receive(mime_type, fd);
    }
}

/// Read clipboard data for all target MIME types from any offer type.
fn read_all_data_formats<O: DataOfferReceive>(
    data_offer: &O,
    mime_types: Vec<String>,
    conn: &Connection,
) -> IndexMap<String, Bytes> {
    use std::io::Read;
    use std::os::fd::AsFd;

    let mut mime_map: IndexMap<String, Bytes> = IndexMap::new();

    if mime_types.is_empty() {
        return mime_map;
    }

    for mime in select_target_mimes(&mime_types) {
        let (reader_fd, writer_fd) = match create_pipes() {
            Ok(pair) => pair,
            Err(err) => {
                warn!("Could not open pipe to read data for {mime}: {err:?}");
                continue;
            }
        };
        debug!("Requesting {mime} content...");
        data_offer.receive_mime(mime.clone(), writer_fd.as_fd());
        // Drop the write end so the provider gets EOF after writing.
        drop(writer_fd);
        if let Err(e) = conn.flush() {
            warn!("Flush failed: {e}");
        }
        let mut reader_file = std::fs::File::from(reader_fd);
        let mut buf = Vec::new();
        match reader_file.read_to_end(&mut buf) {
            Ok(_) if !buf.is_empty() => {
                mime_map.insert(mime, Bytes::from(buf));
            }
            Ok(_) => {}
            Err(e) => warn!("Failed reading data for mime: {e}"),
        }
    }

    mime_map
}
