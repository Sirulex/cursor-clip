use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashMap;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_seat;
use wayland_client::QueueHandle;
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1, 
    zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, 
    zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
    zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
};

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
    pub next_id: u64,
    
    // Wayland objects for clipboard operations
    pub data_control_manager: Option<ZwlrDataControlManagerV1>,
    pub data_control_device: Option<ZwlrDataControlDeviceV1>,
    pub seat: Option<wl_seat::WlSeat>,
    
    // Current clipboard data
    pub offers: HashMap<ObjectId, DataOffer>,
    pub current_selection: Option<DataOffer>,
    pub current_source: Option<ZwlrDataControlSourceV1>,
    pub pending_clipboard_text: Option<String>,
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
            next_id: 1,
            data_control_manager: None,
            data_control_device: None,
            seat: None,
            offers: HashMap::new(),
            current_selection: None,
            current_source: None,
            pending_clipboard_text: None,
        }
    }

    pub fn add_clipboard_item(&mut self, content: String) {
        let item = ClipboardItem {
            id: self.next_id,
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
        self.next_id += 1;
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
        if let Some(item) = self.get_item_by_id(id) {
            println!("Setting clipboard content by ID {}: {}", id, item.content);
            
            // Store the text to be set when the next data source event occurs
            self.pending_clipboard_text = Some(item.content);
            
            // If we have the necessary objects, the event loop will handle creating the source
            // Otherwise, return an error
            if self.data_control_manager.is_some() && self.data_control_device.is_some() {
                Ok(())
            } else {
                Err("Wayland clipboard objects not available".into())
            }
        } else { 
            Err(format!("No clipboard item found with ID: {}", id)) 
        }
    }

    /// Create a new data source for setting clipboard content
    pub fn create_clipboard_source(&mut self, qh: &QueueHandle<BackendState>) -> Result<(), String> {
        if let Some(pending_text) = &self.pending_clipboard_text {
            if let (Some(manager), Some(device)) = (&self.data_control_manager, &self.data_control_device) {
                let source = manager.create_data_source(qh, ());
                source.offer("text/plain".into());
                source.offer("text/plain;charset=utf-8".into());
                
                self.current_source = Some(source.clone());
                device.set_selection(Some(&source));
                
                println!("✅ Created clipboard source for: {}", pending_text);
                return Ok(());
            }
        }
        Err("Cannot create clipboard source".into())
    }
}
