use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashMap;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_seat;
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1, 
    zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, 
    zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
    zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
};
use crate::backend::wayland_clipboard::SharedBackendStateWrapper; // for QueueHandle type
use wayland_client::{QueueHandle, Connection};

use crate::shared::{ClipboardItem, ClipboardContentType};
use indexmap::IndexMap;

#[derive(Debug, Clone)]
pub struct DataOffer {
    pub offer: ZwlrDataControlOfferV1,
    pub mime_types: Vec<String>,
}

#[derive(Debug)]
pub struct BackendState {
    // Clipboard history and management
    pub history: Vec<ClipboardItem>,
    pub id_for_next_entry: u64,
    
    // Wayland objects for clipboard operations
    pub data_control_manager: Option<ZwlrDataControlManagerV1>,
    pub data_control_device: Option<ZwlrDataControlDeviceV1>,
    pub qh: Option<QueueHandle<SharedBackendStateWrapper>>,
    pub seat: Option<wl_seat::WlSeat>,
    pub connection: Option<Connection>,
    
    // Current clipboard data
    pub mime_type_offers: HashMap<ObjectId, DataOffer>,
    pub current_data_offer: Option<DataOffer>,
    pub current_source_object: Option<ZwlrDataControlSourceV1>,
    pub current_source_entry_id: Option<u64>,
    // When we programmatically set the selection, the compositor will echo it
    // back as a new offer/selection. If we immediately try to read that offer
    // inside the dispatch callback, we deadlock because the Send event for our
    // own ZwlrDataControlSourceV1 cannot be processed until we return to the
    // event loop. This flag suppresses reading the very next selection so we
    // avoid blocking on our own source.
    pub suppress_next_selection_read: bool,
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
            id_for_next_entry: 1,
            data_control_manager: None,
            data_control_device: None,
            seat: None,
            mime_type_offers: HashMap::new(),
            current_data_offer: None,
            current_source_object: None,
            current_source_entry_id: None,
            qh: None,
            suppress_next_selection_read: false,
            connection: None,
        }
    }

    pub fn add_clipboard_item_from_mime_map(&mut self, mut indexmap_mime_content: IndexMap<String, Vec<u8>>) {
        if indexmap_mime_content.is_empty() { return; }

        // Preview-Kandidat bestimmen
        let mut preview_source: Option<(String, Vec<u8>)> = None;
        if let Some(v) = indexmap_mime_content.get("text/plain").cloned() { preview_source = Some(("text/plain".into(), v)); }
        if preview_source.is_none() {
            if let Some((k,v)) = indexmap_mime_content.iter().find(|(k,_)| k.starts_with("text/")) { preview_source = Some((k.clone(), v.clone())); }
        }
        let (preview_mime, preview_bytes) = match preview_source { Some(p) => p, None => indexmap_mime_content.iter().next().map(|(k,v)| (k.clone(), v.clone())).unwrap() };
        let content_string = String::from_utf8(preview_bytes.clone()).unwrap_or_else(|_| format!("<{} {} bytes>", preview_mime, preview_bytes.len()));
        let preview = if content_string.len() > 256 { format!("{}…", &content_string[..252]) } else { content_string.clone() };
        let content_type = ClipboardContentType::from_content(&content_string);


        let item = ClipboardItem {
            item_id: self.id_for_next_entry,
            content_type_preview: content_type,
            content_preview: preview,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            mime_data: indexmap_mime_content.drain(..).collect(),
        };

        // remove duplicates (todo change to more robust solution -> hashes)
        self.history.retain(|existing| existing.content_preview != item.content_preview); //
        self.history.insert(0, item);
        if self.history.len() > 100 { self.history.truncate(100); }
        self.id_for_next_entry += 1;
    }

    pub fn get_history(&self) -> Vec<ClipboardItem> { 
        self.history.clone() 
    }
    
    pub fn get_item_by_id(&self, id: u64) -> Option<ClipboardItem> { 
        self.history.iter().find(|i| i.item_id == id).cloned() 
    }
    
    pub fn clear_history(&mut self) { 
        self.history.clear(); 
    }

    pub fn set_clipboard_by_id(&mut self, entry_id: u64) -> Result<(), String> {
        let item = self.get_item_by_id(entry_id).ok_or_else(|| format!("No clipboard item found with ID: {}", entry_id))?;
        println!("Setting clipboard content by ID {}: {}", entry_id, item.content_preview);

        let (manager, device, qh) = match (&self.data_control_manager, &self.data_control_device, &self.qh) {
            (Some(m), Some(d), Some(q)) => (m.clone(), d.clone(), q.clone()),
            _ => return Err("Wayland clipboard objects not available yet".into()),
        };

        let source = manager.create_data_source(&qh, ());
        for (mime, _data) in &item.mime_data { source.offer(mime.clone()); }
        device.set_selection(Some(&source));
        self.current_source_object = Some(source.clone());
        self.current_source_entry_id = Some(entry_id);
        // Prevent reading back our own just-set selection (would deadlock due to event queue handling)
        self.suppress_next_selection_read = true;
        // Flush the Wayland connection so the compositor sees our selection (very important)
        if let Some(conn) = &self.connection {
            if let Err(e) = conn.flush() { eprintln!("Failed to flush Wayland connection after setting selection: {e}"); }
        }
        println!("✅ Created clipboard source and set selection (id {})", entry_id);
        Ok(())
    }
}
