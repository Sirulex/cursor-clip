use std::os::unix::net::UnixStream as StdUnixStream;
use std::io::{BufRead, BufReader, Write};
use tokio::net::UnixStream as TokioUnixStream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use crate::shared::{FrontendMessage, BackendMessage, ClipboardItem};

const SOCKET_PATH: &str = "/tmp/cursor-clip.sock";

/// Unified frontend client that supports both sync and async operations
pub struct FrontendClient {
    sync_stream: Option<StdUnixStream>,
    async_stream: Option<TokioUnixStream>,
}

impl FrontendClient {
    /// Create a new sync client
    pub fn new_sync() -> Result<Self, Box<dyn std::error::Error>> {
        let stream = StdUnixStream::connect(SOCKET_PATH)?;
        Ok(Self {
            sync_stream: Some(stream),
            async_stream: None,
        })
    }

    /// Create a new async client
    pub async fn new_async() -> Result<Self, Box<dyn std::error::Error>> {
        let stream = TokioUnixStream::connect(SOCKET_PATH).await?;
        Ok(Self {
            sync_stream: None,
            async_stream: Some(stream),
        })
    }

    /// Send a message synchronously
    pub fn send_message_sync(&mut self, message: FrontendMessage) -> Result<BackendMessage, Box<dyn std::error::Error>> {
        if let Some(ref mut stream) = self.sync_stream {
            let message_json = serde_json::to_string(&message)?;
            stream.write_all(message_json.as_bytes())?;
            stream.write_all(b"\n")?;

            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line)?;
            
            let response: BackendMessage = serde_json::from_str(&line.trim())?;
            Ok(response)
        } else {
            Err("No sync connection available".into())
        }
    }

    /// Send a message asynchronously
    pub async fn send_message_async(&mut self, message: FrontendMessage) -> Result<BackendMessage, Box<dyn std::error::Error>> {
        if let Some(ref mut stream) = self.async_stream {
            let message_json = serde_json::to_string(&message)?;
            stream.write_all(message_json.as_bytes()).await?;
            stream.write_all(b"\n").await?;

            let mut lines = TokioBufReader::new(stream).lines();
            
            if let Some(line) = lines.next_line().await? {
                let response: BackendMessage = serde_json::from_str(&line)?;
                return Ok(response);
            }
        }
        
        Err("No async connection available".into())
    }

    /// Get clipboard history (sync)
    pub fn get_history_sync(&mut self) -> Result<Vec<ClipboardItem>, Box<dyn std::error::Error>> {
        let response = self.send_message_sync(FrontendMessage::GetHistory)?;
        match response {
            BackendMessage::History { items } => Ok(items),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Get clipboard history (async)
    pub async fn get_history_async(&mut self) -> Result<Vec<ClipboardItem>, Box<dyn std::error::Error>> {
        let response = self.send_message_async(FrontendMessage::GetHistory).await?;
        match response {
            BackendMessage::History { items } => Ok(items),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Set clipboard by content (sync) - legacy method
    pub fn set_clipboard_sync(&mut self, content: String) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message_sync(FrontendMessage::SetClipboard { content })?;
        match response {
            BackendMessage::ClipboardSet => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Set clipboard by content (async) - legacy method
    pub async fn set_clipboard_async(&mut self, content: String) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message_async(FrontendMessage::SetClipboard { content }).await?;
        match response {
            BackendMessage::ClipboardSet => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Set clipboard by ID (sync) - preferred method
    pub fn set_clipboard_by_id_sync(&mut self, id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message_sync(FrontendMessage::SetClipboardById { id })?;
        match response {
            BackendMessage::ClipboardSet => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Set clipboard by ID (async) - preferred method
    pub async fn set_clipboard_by_id_async(&mut self, id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message_async(FrontendMessage::SetClipboardById { id }).await?;
        match response {
            BackendMessage::ClipboardSet => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Clear history (sync)
    pub fn clear_history_sync(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message_sync(FrontendMessage::ClearHistory)?;
        match response {
            BackendMessage::HistoryCleared => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    /// Clear history (async)
    pub async fn clear_history_async(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message_async(FrontendMessage::ClearHistory).await?;
        match response {
            BackendMessage::HistoryCleared => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }
}

/// Convenience alias for the old sync client
pub type SyncFrontendClient = FrontendClient;

impl SyncFrontendClient {
    /// Compatibility method for old API
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_sync()
    }

    /// Compatibility method for old API
    pub fn send_message(&mut self, message: FrontendMessage) -> Result<BackendMessage, Box<dyn std::error::Error>> {
        self.send_message_sync(message)
    }

    /// Compatibility method for old API
    pub fn get_history(&mut self) -> Result<Vec<ClipboardItem>, Box<dyn std::error::Error>> {
        self.get_history_sync()
    }

    /// Compatibility method for old API
    pub fn set_clipboard(&mut self, content: String) -> Result<(), Box<dyn std::error::Error>> {
        self.set_clipboard_sync(content)
    }

    /// New preferred method - set clipboard by ID
    pub fn set_clipboard_by_id(&mut self, id: u64) -> Result<(), Box<dyn std::error::Error>> {
        self.set_clipboard_by_id_sync(id)
    }

    /// Compatibility method for old API
    pub fn clear_history(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.clear_history_sync()
    }
}
