use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashMap;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_seat;
use crate::backend::wayland_clipboard::MutexBackendState;
use wayland_client::{QueueHandle, Connection};

// Import both protocol types
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
    zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
};

use crate::backend::ext_data_control::{
    ExtDataControlManagerV1,
    ExtDataControlDeviceV1,
    ExtDataControlSourceV1,
};

use crate::shared::{ClipboardItem, ClipboardItemPreview, ClipboardContentType};
use indexmap::IndexMap;
use bytes::Bytes;
use log::{debug, info, warn};

/// Which data control protocol is being used
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DataControlProtocol {
    /// wlroots protocol (zwlr_data_control_manager_v1)
    Wlr,
    /// Standard protocol (ext_data_control_manager_v1)
    Ext,
}

#[derive(Debug)]
pub struct BackendState {
    // Clipboard history and management
    pub history: Vec<ClipboardItem>,
    pub id_for_next_entry: u64,

    // Which protocol is active
    pub active_protocol: Option<DataControlProtocol>,

    // Wayland objects for clipboard operations - wlroots
    pub data_control_manager: Option<ZwlrDataControlManagerV1>,
    pub data_control_device: Option<ZwlrDataControlDeviceV1>,

    // Wayland objects for clipboard operations - ext (standard)
    pub ext_data_control_manager: Option<ExtDataControlManagerV1>,
    pub ext_data_control_device: Option<ExtDataControlDeviceV1>,

    pub qh: Option<QueueHandle<MutexBackendState>>,
    pub seat: Option<wl_seat::WlSeat>,
    pub connection: Option<Connection>,

    // Current clipboard data - wlroots
    pub mime_type_offers: HashMap<ObjectId, Vec<String>>,
    pub current_data_offer: Option<ObjectId>,
    pub current_source_object: Option<ZwlrDataControlSourceV1>,
    pub current_source_entry_id: Option<u64>,

    // Current clipboard data - ext
    pub ext_mime_type_offers: HashMap<ObjectId, Vec<String>>,
    pub ext_current_data_offer: Option<ObjectId>,
    pub ext_current_source_object: Option<ExtDataControlSourceV1>,
    pub ext_current_source_entry_id: Option<u64>,

    // When we programmatically set the selection, the compositor will echo it
    // back as a new offer/selection. If we immediately try to read that offer
    // inside the dispatch callback, we deadlock because the Send event for our
    // own source cannot be processed until we return to the event loop.
    pub suppress_next_selection_read: bool,
    /// If true, we only monitor external selections and DO NOT immediately
    /// re-set (take ownership of) the newly received selection.
    pub monitor_only: bool,
}

impl Default for BackendState {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendState {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            mime_type_offers: HashMap::new(),
            ext_mime_type_offers: HashMap::new(),
            id_for_next_entry: 1,
            active_protocol: None,
            data_control_manager: None,
            data_control_device: None,
            ext_data_control_manager: None,
            ext_data_control_device: None,
            seat: None,
            current_data_offer: None,
            ext_current_data_offer: None,
            current_source_object: None,
            ext_current_source_object: None,
            current_source_entry_id: None,
            ext_current_source_entry_id: None,
            qh: None,
            suppress_next_selection_read: false,
            connection: None,
            monitor_only: false,
        }
    }

    pub fn add_clipboard_item_from_mime_map(&mut self, mut mime_content: IndexMap<String, Bytes>) -> Option<u64> {
        if mime_content.is_empty() { return None; }

        // If we have image/png, prefer showing mime_type + bytes and set type to Image
        let (content_preview, content_type) = if let Some(png_bytes) = mime_content.get("image/png") {
            (format!("<image/png {} bytes>", png_bytes.len()), ClipboardContentType::Image)
        } else {
            // Otherwise, if we have text/plain;charset=utf-8, show up to first 200 chars and infer type
            let preview: String = if let Some(txt_bytes) = mime_content.get("text/plain;charset=utf-8") {
                match std::str::from_utf8(txt_bytes.as_ref()) {
                    Ok(s) => s.chars().take(200).collect(),
                    Err(_) => format!("<text/plain;charset=utf-8 {} bytes>", txt_bytes.len()),
                }
            } else {
                // Fallback: show placeholder using first mime entry
                let (mime_name, len) = mime_content.iter().next().map(|(k,v)| (k.clone(), v.len())).unwrap();
                format!("<{mime_name} {len} bytes>")
            };
            let content_type = ClipboardContentType::type_from_preview(&preview);
            (preview, content_type)
        };


        let item = ClipboardItem {
            item_id: self.id_for_next_entry,
            content_type,
            content_preview,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            mime_data: mime_content.drain(..).collect(),
        };

        // remove duplicates (todo change to more robust solution -> hashes)
        self.history.retain(|existing| existing.content_preview != item.content_preview);
        self.history.insert(0, item);
        if self.history.len() > 100 { self.history.truncate(100); }
        let new_id = self.id_for_next_entry;
        self.id_for_next_entry += 1;
        Some(new_id)
    }

    pub fn get_history(&self) -> Vec<ClipboardItemPreview> {
    self.history.iter().map(ClipboardItemPreview::from).collect()
    }

    pub fn get_item_by_id(&self, id: u64) -> Option<ClipboardItem> {
        self.history.iter().find(|i| i.item_id == id).cloned()
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    pub fn set_clipboard_by_id(&mut self, entry_id: u64) -> Result<(), String> {
        let item = self.get_item_by_id(entry_id).ok_or_else(|| format!("No clipboard item found with ID: {entry_id}"))?;

        info!("Setting clipboard content by ID {entry_id}");
        debug!("Setting clipboard content by ID {entry_id}: {}", item.content_preview);

        match self.active_protocol {
            Some(DataControlProtocol::Wlr) => self.set_clipboard_wlr(entry_id, &item),
            Some(DataControlProtocol::Ext) => self.set_clipboard_ext(entry_id, &item),
            None => Err("No data control protocol available".into()),
        }
    }

    fn set_clipboard_wlr(&mut self, entry_id: u64, item: &ClipboardItem) -> Result<(), String> {
        let (Some(manager), Some(device), Some(qh)) = (
            &self.data_control_manager,
            &self.data_control_device,
            &self.qh,
        ) else {
            return Err("Wayland wlroots clipboard objects not available yet".into());
        };

        // Clean up any previously set source that we own
        if let Some(prev) = self.current_source_object.take() {
            prev.destroy();
        }

        let source = manager.create_data_source(qh, ());
        for (mime, _data) in &item.mime_data { source.offer(mime.clone()); }
        device.set_selection(Some(&source));
        self.current_source_object = Some(source);
        self.current_source_entry_id = Some(entry_id);
        self.suppress_next_selection_read = true;

        if let Some(conn) = &self.connection {
            if let Err(e) = conn.flush() { warn!("Failed to flush Wayland connection after setting selection: {e}"); }
        }
        debug!("Created wlroots clipboard source and set selection (id {entry_id})");
        Ok(())
    }

    fn set_clipboard_ext(&mut self, entry_id: u64, item: &ClipboardItem) -> Result<(), String> {
        let (Some(manager), Some(device), Some(qh)) = (
            &self.ext_data_control_manager,
            &self.ext_data_control_device,
            &self.qh,
        ) else {
            return Err("Wayland ext clipboard objects not available yet".into());
        };

        // Clean up any previously set source that we own
        if let Some(prev) = self.ext_current_source_object.take() {
            prev.destroy();
        }

        let source = manager.create_data_source(qh, ());
        for (mime, _data) in &item.mime_data { source.offer(mime.clone()); }
        device.set_selection(Some(&source));
        self.ext_current_source_object = Some(source);
        self.ext_current_source_entry_id = Some(entry_id);
        self.suppress_next_selection_read = true;

        if let Some(conn) = &self.connection {
            if let Err(e) = conn.flush() { warn!("Failed to flush Wayland connection after setting selection: {e}"); }
        }
        debug!("Created ext clipboard source and set selection (id {entry_id})");
        Ok(())
    }
}
