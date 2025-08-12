use std::os::unix::net::UnixStream;
use std::io::{BufRead, BufReader, Write};
use crate::shared::{FrontendMessage, BackendMessage, ClipboardItem};

pub struct SyncFrontendClient {
    stream: UnixStream,
}

impl SyncFrontendClient {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let stream = UnixStream::connect("/tmp/cursor-clip.sock")?;
        Ok(Self { stream })
    }

    pub fn send_message(&mut self, message: FrontendMessage) -> Result<BackendMessage, Box<dyn std::error::Error>> {
        let message_json = serde_json::to_string(&message)?;
        self.stream.write_all(message_json.as_bytes())?;
        self.stream.write_all(b"\n")?;

        let mut reader = BufReader::new(&self.stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        
        let response: BackendMessage = serde_json::from_str(&line.trim())?;
        Ok(response)
    }

    pub fn get_history(&mut self) -> Result<Vec<ClipboardItem>, Box<dyn std::error::Error>> {
        let response = self.send_message(FrontendMessage::GetHistory)?;
        match response {
            BackendMessage::History { items } => Ok(items),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    pub fn set_clipboard(&mut self, content: String) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message(FrontendMessage::SetClipboard { content })?;
        match response {
            BackendMessage::ClipboardSet => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }

    pub fn clear_history(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message(FrontendMessage::ClearHistory)?;
        match response {
            BackendMessage::HistoryCleared => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }
}
