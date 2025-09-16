use std::sync::{Arc, Mutex};
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::shared::{BackendMessage, FrontendMessage};
use super::wayland_clipboard::WaylandClipboardMonitor;
use super::backend_state::BackendState;
use log::{info, error};
use bytes::Bytes;

pub async fn run_backend(monitor_only: bool) -> Result<(), Box<dyn std::error::Error>> { 
    // Remove existing socket if it exists
    let socket_path = "/tmp/cursor-clip.sock";
    let _ = std::fs::remove_file(socket_path);

    // Create Unix socket for IPC
    let listener = UnixListener::bind(socket_path)?;
    info!("Clipboard backend listening on {}", socket_path);

    let state = Arc::new(Mutex::new(BackendState::new()));
    {
        let mut s = state.lock().unwrap();
    s.monitor_only = monitor_only;
    }

    // Start Wayland clipboard monitoring in a separate task
    let wayland_state = state.clone();
    tokio::spawn(async move {
        match WaylandClipboardMonitor::new(wayland_state) {
            Ok(mut monitor) => {
                if let Err(e) = monitor.start_monitoring().await {
                    error!("Wayland clipboard monitoring error: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to create Wayland clipboard monitor: {}", e);
            }
        }
    });

    // Add some sample data only in debug builds (helps during development without polluting release)
    #[cfg(debug_assertions)]
    {
        let mut state_lock = state.lock().unwrap();
        use indexmap::IndexMap;
        for sample in [
            "Hello, world Cursor-Clip!",
            "https://github.com/rust-lang/rust",
            "Sample clipboard content for testing the clipboard manager",
            "impl Display for MyStruct {\n    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {\n        write!(f, \"MyStruct\")\n    }\n}",
            "Password4234!Cursor-Clip",
        ] {
            let mut map = IndexMap::new();
            map.insert("text/plain;charset=utf-8".to_string(), Bytes::from_static(sample.as_bytes()));
            let _ = state_lock.add_clipboard_item_from_mime_map(map);
        }
    }

    // Handle IPC connections
    loop {
        let (stream, _addr) = listener.accept().await?;
        let state_clone = state.clone();
        
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, state_clone).await {
                error!("Client error: {}", e);
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
                let mut state = state.lock().unwrap();
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
        };

        let response_json = serde_json::to_string(&response)?;
        writer.write_all(response_json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
    }

    Ok(())
}
