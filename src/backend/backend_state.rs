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
    pub seat: Option<wl_seat::WlSeat>,
    
    // Current clipboard data
    pub offers: HashMap<ObjectId, DataOffer>,
    pub current_selection: Option<DataOffer>,
    pub current_source_object: Option<ZwlrDataControlSourceV1>,
    pub current_source_id: Option<u64>,
    // Queue handle (for creating data sources) tied to wrapper state type
    pub qh: Option<QueueHandle<SharedBackendStateWrapper>>,
    // When we programmatically set the selection, the compositor will echo it
    // back as a new offer/selection. If we immediately try to read that offer
    // inside the dispatch callback, we deadlock because the Send event for our
    // own ZwlrDataControlSourceV1 cannot be processed until we return to the
    // event loop. This flag suppresses reading the very next selection so we
    // avoid blocking on our own source.
    pub suppress_next_selection_read: bool,
    // Connection handle so we can flush after setting a selection
    pub connection: Option<Connection>,
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
            offers: HashMap::new(),
            current_selection: None,
            current_source_object: None,
            current_source_id: None,
            qh: None,
            suppress_next_selection_read: false,
            connection: None,
        }
    }

    pub fn add_clipboard_item(&mut self, content: String) {
        let item = ClipboardItem {
            id: self.id_for_next_entry,
            content_type: ClipboardContentType::from_content(&content),
            content,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        // Remove previous occurrence of identical content
        self.history.retain(|existing| existing.content != item.content);
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
        self.history.iter().find(|i| i.id == id).cloned() 
    }
    
    pub fn clear_history(&mut self) { 
        self.history.clear(); 
    }

    pub fn set_clipboard_by_id(&mut self, id: u64) -> Result<(), String> {
        let item = self.get_item_by_id(id).ok_or_else(|| format!("No clipboard item found with ID: {}", id))?;
        println!("Setting clipboard content by ID {}: {}", id, item.content);

        let (manager, device, qh) = match (&self.data_control_manager, &self.data_control_device, &self.qh) {
            (Some(m), Some(d), Some(q)) => (m.clone(), d.clone(), q.clone()),
            _ => return Err("Wayland clipboard objects not available yet".into()),
        };

        let source = manager.create_data_source(&qh, ());
        source.offer("text/plain".into());
        device.set_selection(Some(&source));
        self.current_source_object = Some(source);
        self.current_source_id = Some(id);
        // Prevent reading back our own just-set selection (would deadlock)
        self.suppress_next_selection_read = true;
        // Flush the Wayland connection so the compositor sees our selection (very important)
        if let Some(conn) = &self.connection {
            if let Err(e) = conn.flush() { eprintln!("Failed to flush Wayland connection after setting selection: {e}"); }
        }
        println!("✅ Created clipboard source and set selection (id {})", id);
        Ok(())
    }
}
