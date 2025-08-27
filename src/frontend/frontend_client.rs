use std::os::unix::net::UnixStream;
use std::io::{BufRead, BufReader, Write};
use crate::shared::{FrontendMessage, BackendMessage, ClipboardItem};

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
    pub fn send_message_sync(&mut self, message: FrontendMessage) -> Result<BackendMessage, Box<dyn std::error::Error>> {
        let message_json = serde_json::to_string(&message)?;
        self.stream.write_all(message_json.as_bytes())?;
        self.stream.write_all(b"\n")?;

        let mut reader = BufReader::new(&self.stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        
        let response: BackendMessage = serde_json::from_str(&line.trim())?;
        Ok(response)
    }

    /// Get clipboard history
    pub fn get_history(&mut self) -> Result<Vec<ClipboardItem>, Box<dyn std::error::Error>> {
        let response = self.send_message_sync(FrontendMessage::GetHistory)?;
        match response {
            BackendMessage::History { items } => Ok(items),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Set clipboard by ID (preferred method)
    pub fn set_clipboard_by_id(&mut self, id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message_sync(FrontendMessage::SetClipboardById { id })?;
        match response {
            BackendMessage::ClipboardSet => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Clear history
    pub fn clear_history(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message_sync(FrontendMessage::ClearHistory)?;
        match response {
            BackendMessage::HistoryCleared => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }
}

/// Convenience alias for backward compatibility
pub type SyncFrontendClient = FrontendClient;

impl SyncFrontendClient {
    /// Send a message and get response (compatibility method)
    pub fn send_message(&mut self, message: FrontendMessage) -> Result<BackendMessage, Box<dyn std::error::Error>> {
        self.send_message_sync(message)
    }
}
