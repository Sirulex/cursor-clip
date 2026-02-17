use crate::shared::{BackendMessage, ClipboardItemPreview, FrontendMessage};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

const SOCKET_PATH: &str = "/tmp/cursor-clip.sock";

/// Frontend client for communicating with the backend
pub struct FrontendClient {
    stream: UnixStream,
}

impl FrontendClient {
    /// Create a new client
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let stream = UnixStream::connect(SOCKET_PATH)?;
        Ok(Self { stream })
    }

    /// Send a message and get response
    pub fn send_message(
        &mut self,
        message: FrontendMessage,
    ) -> Result<BackendMessage, Box<dyn std::error::Error>> {
        let message_json = serde_json::to_string(&message)?;
        self.stream.write_all(message_json.as_bytes())?;
        self.stream.write_all(b"\n")?;

        let mut reader = BufReader::new(&self.stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;

        let response: BackendMessage = serde_json::from_str(line.trim())?;
        Ok(response)
    }

    /// Get clipboard history
    pub fn get_history(&mut self) -> Result<Vec<ClipboardItemPreview>, Box<dyn std::error::Error>> {
        let response = self.send_message(FrontendMessage::GetHistory)?;
        match response {
            BackendMessage::History { items } => Ok(items),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Set clipboard by ID
    pub fn set_clipboard_by_id(&mut self, id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message(FrontendMessage::SetClipboardById { id })?;
        match response {
            BackendMessage::ClipboardSet => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Set pinned state by ID
    pub fn set_pinned(&mut self, id: u64, pinned: bool) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message(FrontendMessage::SetPinned { id, pinned })?;
        match response {
            BackendMessage::ItemPinned { .. } => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Clear history
    pub fn clear_history(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message(FrontendMessage::ClearHistory)?;
        match response {
            BackendMessage::HistoryCleared => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Delete a single clipboard item by ID
    pub fn delete_item_by_id(&mut self, id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message(FrontendMessage::DeleteItemById { id })?;
        match response {
            BackendMessage::ItemDeleted { .. } => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }
}
