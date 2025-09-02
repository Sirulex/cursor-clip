use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wayland_client::{Connection, protocol::wl_registry};

use crate::shared::{BackendMessage, FrontendMessage, ClipboardItem, ClipboardContentType};
use super::wayland_clipboard::{WaylandClipboardMonitor, create_selection};

#[derive(Debug, Default)]
pub struct BackendState {
    history: Vec<ClipboardItem>,
    next_id: u64,
}

impl BackendState {
    pub fn new() -> Self { Self { history: Vec::new(), next_id: 1 } }

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
        if self.history.len() > 100 { self.history.truncate(100); }
        self.next_id += 1;
    }

    pub fn get_history(&self) -> Vec<ClipboardItem> { self.history.clone() }
    pub fn get_item_by_id(&self, id: u64) -> Option<ClipboardItem> { self.history.iter().find(|i| i.id == id).cloned() }
    pub fn clear_history(&mut self) { self.history.clear(); }
    pub fn set_clipboard_by_id(&self, id: u64) -> Result<(), String> {
        if let Some(item) = self.get_item_by_id(id) {
            println!("Setting clipboard content by ID {}: {}", id, item.content);
            // TEST: also spawn a fixed selection provider using create_selection
            if let Err(e) = create_selection() { eprintln!("Failed to start test selection provider: {e}"); }
            
            Ok(())
        } else { Err(format!("No clipboard item found with ID: {}", id)) }
    }
}

// Wayland registry dispatch implementation
impl wayland_client::Dispatch<wl_registry::WlRegistry, ()> for BackendState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: <wl_registry::WlRegistry as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &wayland_client::QueueHandle<Self>,
    ) {
        // Handle registry events for backend
    }
}

pub async fn run_backend() -> Result<(), Box<dyn std::error::Error>> { 
    // Remove existing socket if it exists
    let socket_path = "/tmp/cursor-clip.sock";
    let _ = std::fs::remove_file(socket_path);

    // (Removed) shared Wayland connection initialization: backend now creates its own connection inside the clipboard monitor

    // Create Unix socket for IPC
    let listener = UnixListener::bind(socket_path)?;
    println!("Clipboard backend listening on {}", socket_path);

    // Simple state for testing
    let state = Arc::new(Mutex::new(BackendState::new()));

    // Start Wayland clipboard monitoring in a separate task
    let wayland_state = state.clone();
    tokio::spawn(async move {
        match WaylandClipboardMonitor::new(wayland_state) {
            Ok(mut monitor) => {
                if let Err(e) = monitor.start_monitoring().await {
                    eprintln!("Wayland clipboard monitoring error: {}", e);
                }
            }
            Err(e) => {
                eprintln!("Failed to create Wayland clipboard monitor: {}", e);
                println!("Continuing without Wayland clipboard monitoring...");
            }
        }
    });

    // Add some sample data
    {
        let mut state_lock = state.lock().unwrap();
        for sample in [
            "Hello, world Jannik!",
            "https://github.com/rust-lang/rust",
            "Sample clipboard content for testing the clipboard manager",
            "impl Display for MyStruct {\n    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {\n        write!(f, \"MyStruct\")\n    }\n}",
            "Password4234!Jannik",
        ] { state_lock.add_clipboard_item(sample.to_string()); }
    }

    // Handle IPC connections
    loop {
        let (stream, _addr) = listener.accept().await?;
        let state_clone = state.clone();
        
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, state_clone).await {
                eprintln!("Client error: {}", e);
            }
        });
    }
}

async fn handle_client(
    stream: UnixStream,
    state: Arc<Mutex<BackendState>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let message: FrontendMessage = serde_json::from_str(&line)?;
        
        let response = match message {
            FrontendMessage::GetHistory => {
                let state = state.lock().unwrap();
                BackendMessage::History { items: state.get_history() }
            }
            FrontendMessage::SetClipboardById { id } => {
                let state = state.lock().unwrap();
                match state.set_clipboard_by_id(id) {
                    Ok(_) => BackendMessage::ClipboardSet,
                    Err(e) => BackendMessage::Error { message: e },
                }
            }
            FrontendMessage::ClearHistory => {
                let mut state = state.lock().unwrap();
                state.clear_history();
                BackendMessage::HistoryCleared
            }
            FrontendMessage::ShowAt { .. } | FrontendMessage::Close => {
                // These are handled by the frontend, not the backend
                continue;
            }
        };

        let response_json = serde_json::to_string(&response)?;
        writer.write_all(response_json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
    }

    Ok(())
}
