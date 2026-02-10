use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashMap;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_seat;
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
    zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
};
use crate::backend::wayland_clipboard::MutexBackendState; // for QueueHandle type
use wayland_client::{QueueHandle, Connection};

use crate::shared::{ClipboardItem, ClipboardItemPreview, ClipboardContentType};
use indexmap::IndexMap;
use bytes::Bytes;
use log::{debug, info, warn};

#[derive(Debug)]
pub struct BackendState {
    // Clipboard history and management
    pub history: Vec<ClipboardItem>,
    pub id_for_next_entry: u64,
    
    // Wayland objects for clipboard operations
    pub data_control_manager: Option<ZwlrDataControlManagerV1>,
    pub data_control_device: Option<ZwlrDataControlDeviceV1>,
    pub qh: Option<QueueHandle<MutexBackendState>>,
    pub seat: Option<wl_seat::WlSeat>,
    pub connection: Option<Connection>,
    
    // Current clipboard data
    // Mapping of offer ObjectId -> list of MIME types provided by that offer
    pub mime_type_offers: HashMap<ObjectId, Vec<String>>,
    // Currently selected offer id (if any)
    pub current_data_offer: Option<ObjectId>,
    pub current_source_object: Option<ZwlrDataControlSourceV1>,
    pub current_source_entry_id: Option<u64>,
    // When we programmatically set the selection, the compositor will echo it
    // back as a new offer/selection. If we immediately try to read that offer
    // inside the dispatch callback, we deadlock because the Send event for our
    // own ZwlrDataControlSourceV1 cannot be processed until we return to the
    // event loop. This flag suppresses reading the very next selection so we
    // avoid blocking on our own source.
    pub suppress_next_selection_read: bool,
    // If true, we only monitor external selections and DO NOT immediately
    // re-set (take ownership of) the newly received selection.
    // If false (default), after reading an external selection we immediately
    // set it ourselves so it persists even if the source app exits.
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
            id_for_next_entry: 1,
            data_control_manager: None,
            data_control_device: None,
            seat: None,
            current_data_offer: None,
            current_source_object: None,
            current_source_entry_id: None,
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
            pinned: false,
            mime_data: mime_content.drain(..).collect(),
        };

        // remove duplicates (todo change to more robust solution -> hashes)
        self.history.retain(|existing| existing.content_preview != item.content_preview);
        let insert_index = self
            .history
            .iter()
            .position(|existing| !existing.pinned)
            .unwrap_or(self.history.len());
        self.history.insert(insert_index, item);
        if self.history.len() > 100 { self.history.truncate(100); }
        let new_id = self.id_for_next_entry;
        self.id_for_next_entry += 1;
        Some(new_id)
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub fn add_clipboard_item_from_text(&mut self, text: &str) -> Option<u64> {
        let mut mime_content = IndexMap::new();
        mime_content.insert(
            "text/plain;charset=utf-8".to_string(),
            Bytes::copy_from_slice(text.as_bytes()),
        );
        self.add_clipboard_item_from_mime_map(mime_content)
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

    pub fn delete_item_by_id(&mut self, entry_id: u64) -> Result<(), String> {
        let index = self
            .history
            .iter()
            .position(|item| item.item_id == entry_id)
            .ok_or_else(|| format!("No clipboard item found with ID: {entry_id}"))?;

        self.history.remove(index);

        if self.current_source_entry_id == Some(entry_id) {
            if let Some(prev) = self.current_source_object.take() {
                prev.destroy();
            }
            self.current_source_entry_id = None;
        }

        Ok(())
    }

    pub fn set_clipboard_by_id(&mut self, entry_id: u64) -> Result<(), String> {
        let item = self.get_item_by_id(entry_id).ok_or_else(|| format!("No clipboard item found with ID: {entry_id}"))?;
        
        info!("Setting clipboard content by ID {entry_id}");
        debug!("Setting clipboard content by ID {entry_id}: {}", item.content_preview);

        let (Some(manager), Some(device), Some(qh)) = (
            &self.data_control_manager,
            &self.data_control_device,
            &self.qh,
        ) else {
            return Err("Wayland clipboard objects not available yet".into());
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
        // Prevent reading back our own just-set selection (would deadlock due to event queue handling)
        self.suppress_next_selection_read = true;
        // Flush the Wayland connection so the compositor sees our selection (very important)
        if let Some(conn) = &self.connection
            && let Err(e) = conn.flush() {
            warn!("Failed to flush Wayland connection after setting selection: {e}");
        }
        debug!("Created clipboard source and set selection (id {entry_id})");
        Ok(())
    }

    pub fn set_pinned(&mut self, entry_id: u64, pinned: bool) -> Result<(), String> {
        let index = self
            .history
            .iter()
            .position(|item| item.item_id == entry_id)
            .ok_or_else(|| format!("No clipboard item found with ID: {entry_id}"))?;

        let mut item = self.history.remove(index);
        item.pinned = pinned;

        let insert_index = if pinned {
            0
        } else {
            self.history
                .iter()
                .position(|existing| !existing.pinned)
                .unwrap_or(self.history.len())
        };

        self.history.insert(insert_index, item);
        Ok(())
    }
}
