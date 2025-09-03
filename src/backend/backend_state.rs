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
use wayland_client::{QueueHandle, Connection, Proxy};

use crate::shared::{ClipboardItem, ClipboardContentType};

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

    pub fn add_clipboard_item(&mut self, content: String) {
        use indexmap::IndexMap;
        let mut mime_data: IndexMap<String, Vec<u8>> = IndexMap::new();
        // Standard Text-Repräsentation ablegen (UTF-8)
        mime_data.insert("text/plain".to_string(), content.as_bytes().to_vec());
        let preview = if content.len() > 256 { // Preview nicht zu groß machen
            format!("{}…", &content[..252])
        } else { content.clone() };
        let item = ClipboardItem {
            item_id: self.id_for_next_entry,
            content_type_preview: ClipboardContentType::from_content(&content),
            content_preview: preview,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            mime_data,
        };

        // Remove previous occurrence of identical content
        self.history.retain(|existing| existing.content_preview != item.content_preview);
        self.history.insert(0, item);
        if self.history.len() > 100 { 
            self.history.truncate(100); 
        }
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
        source.offer("text/plain".into());
        // Alle bekannten MIME-Typen des Items anbieten (Reihenfolge bleibt erhalten)
        //for (mime, _data) in &item.mime_data {
        //    source.offer(mime.clone());
        //}
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
