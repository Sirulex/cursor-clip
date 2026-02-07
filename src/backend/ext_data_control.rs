// Generated protocol bindings for ext-data-control-v1
// This is the standard protocol supported by KDE Plasma 6

pub mod ext_data_control {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/ext-data-control-v1.xml");
    }

    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/ext-data-control-v1.xml");
}

// Re-export main types for convenience
pub use ext_data_control::ext_data_control_manager_v1::ExtDataControlManagerV1;
pub use ext_data_control::ext_data_control_device_v1::ExtDataControlDeviceV1;
pub use ext_data_control::ext_data_control_source_v1::ExtDataControlSourceV1;
pub use ext_data_control::ext_data_control_offer_v1::ExtDataControlOfferV1;

// Import necessary types for Dispatch implementations
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use std::os::fd::AsFd;
use std::io::Read;
use indexmap::IndexMap;
use bytes::Bytes;
use log::{debug, warn, error};
use crate::backend::backend_state::BackendState;
use crate::backend::wayland_clipboard::MutexBackendState;

// Helper function for creating pipes
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

// ================= Dispatch Implementations for ext protocol =================

use ext_data_control::ext_data_control_device_v1;
use ext_data_control::ext_data_control_offer_v1;
use ext_data_control::ext_data_control_source_v1;

impl Dispatch<ExtDataControlManagerV1, ()> for MutexBackendState {
    fn event(
        _state: &mut Self,
        _proxy: &ExtDataControlManagerV1,
        _event: <ExtDataControlManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        // No events for manager
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: <ExtDataControlDeviceV1 as Proxy>::Event,
        (): &(),
        conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use crate::backend::backend_state::DataControlProtocol;
        let mut state = wrapper.backend_state.lock().unwrap();

        // Only process if we're using ext protocol
        if state.active_protocol != Some(DataControlProtocol::Ext) {
            return;
        }

        match event {
            ext_data_control_device_v1::Event::DataOffer { id } => {
                let object_id = id.id();
                debug!("[EXT] New data offer received with ID: {:?}", object_id);
                state.ext_mime_type_offers.insert(object_id, Vec::new());
            }
            ext_data_control_device_v1::Event::Selection { id } => {
                if let Some(offer_id) = id {
                    let offer_key = offer_id.id();
                    debug!("[EXT] Selection changed to offer ID: {:?}", offer_key);

                    let already_current = state.ext_current_data_offer.as_ref().is_some_and(|o| o == &offer_key);
                    if let Some(mime_list) = state.ext_mime_type_offers.get(&offer_key).cloned() {
                        debug!("[EXT] New clipboard content available with {} MIME types", mime_list.len());
                        if state.suppress_next_selection_read {
                            state.ext_current_data_offer = Some(offer_key);
                            debug!("[EXT] Suppressed reading our own just-set selection");
                            offer_id.destroy();
                        } else if !already_current {
                            state.ext_current_data_offer = Some(offer_key);
                            process_all_data_formats_ext(&offer_id, mime_list, conn, &mut state);
                            state.ext_mime_type_offers.clear();
                            offer_id.destroy();
                        }
                    }
                } else {
                    debug!("[EXT] Selection cleared");
                    state.ext_current_data_offer = None;
                }
            }
            ext_data_control_device_v1::Event::PrimarySelection { .. } => {
                // We ignore primary selection
            }
            ext_data_control_device_v1::Event::Finished => {
                debug!("[EXT] Data control device finished");
            }
        }
    }

    fn event_created_child(
        opcode: u16,
        qhandle: &QueueHandle<Self>,
    ) -> std::sync::Arc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            0 => {
                // DataOffer event - create a data offer object data
                qhandle.make_data::<ExtDataControlOfferV1, ()>(())
            }
            _ => panic!("Unknown child object for opcode {opcode}"),
        }
    }
}

impl Dispatch<ExtDataControlOfferV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        offer: &ExtDataControlOfferV1,
        event: <ExtDataControlOfferV1 as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // ext_data_control_offer_v1 only has one event: Offer
        let ext_data_control_offer_v1::Event::Offer { mime_type } = event;
        let object_id = offer.id();
        debug!("[EXT] Offer event: MIME type offered: {}", mime_type);
        let mut state = wrapper.backend_state.lock().unwrap();
        if let Some(mime_list) = state.ext_mime_type_offers.get_mut(&object_id) {
            if !mime_type.starts_with("video") {
                mime_list.push(mime_type);
            }
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for MutexBackendState {
    fn event(
        wrapper: &mut Self,
        event_source: &ExtDataControlSourceV1,
        event: <ExtDataControlSourceV1 as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut state = wrapper.backend_state.lock().unwrap();

        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                debug!("[EXT] Data source Send event for MIME type: {}", mime_type);
                if let Some(item_id) = state.ext_current_source_entry_id {
                    if let Some(item) = state.get_item_by_id(item_id) {
                        use std::io::Write;
                        let mut file: std::fs::File = fd.into();
                        if let Some(bytes) = item.mime_data.get(&mime_type) {
                            if let Err(e) = file.write_all(bytes.as_ref()) {
                                error!(
                                    "[EXT] Failed writing selection data (id {}, mime {}): {}",
                                    item_id, mime_type, e
                                );
                            } else {
                                debug!("[EXT] Wrote {} bytes for id {} (mime {})", bytes.len(), item_id, mime_type);
                            }
                        } else {
                            warn!("[EXT] No data stored for MIME {} (id {})", mime_type, item_id);
                        }
                    } else {
                        warn!("[EXT] Clipboard item id {} no longer exists", item_id);
                    }
                } else {
                    warn!("[EXT] No current_source_id set when Send event received");
                }
            }
            ext_data_control_source_v1::Event::Cancelled => {
                debug!("[EXT] Data source cancelled");
                if state.ext_current_source_object.as_ref().map(Proxy::id) == Some(event_source.id()) {
                    state.suppress_next_selection_read = false;
                    state.ext_current_source_object = None;
                    debug!("[EXT] Re-enabled selection reading");
                }
                drop(state);
                event_source.destroy();
            }
        }
    }
}

fn process_all_data_formats_ext(
    data_offer: &ExtDataControlOfferV1,
    mime_types: Vec<String>,
    conn: &Connection,
    backend_state: &mut BackendState,
) {
    if mime_types.is_empty() { return; }

    let mut mime_map: IndexMap<String, Bytes> = IndexMap::new();

    for mime in mime_types {
        let (reader_fd, writer_fd) = match create_pipes() {
            Ok(pair) => pair,
            Err(err) => { warn!("[EXT] Could not open pipe to read data for {}: {:?}", mime, err); continue; }
        };
        debug!("[EXT] Requesting {} content...", mime);
        data_offer.receive(mime.clone(), writer_fd.as_fd());
        drop(writer_fd);
        if let Err(e) = conn.flush() { warn!("[EXT] Flush failed: {}", e); }

        let mut reader_file = std::fs::File::from(reader_fd);
        let mut buf = Vec::new();
        match reader_file.read_to_end(&mut buf) {
            Ok(_) => {
                if !buf.is_empty() { mime_map.insert(mime, Bytes::from(buf)); }
            }
            Err(e) => warn!("[EXT] Failed reading data for mime: {}", e),
        }
    }

    if !mime_map.is_empty() {
        if let Some(new_id) = backend_state.add_clipboard_item_from_mime_map(mime_map) {
            if !backend_state.monitor_only && !backend_state.suppress_next_selection_read {
                if let Err(e) = backend_state.set_clipboard_by_id(new_id) {
                    warn!("[EXT] Failed to take ownership of selection id {}: {}", new_id, e);
                } else {
                    debug!("[EXT] Took ownership of external selection (id {})", new_id);
                }
            }
        }
    }
}
