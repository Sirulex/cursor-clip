use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use super::backend_state::{BackendState, unix_timestamp};
use super::wayland_clipboard::WaylandClipboardMonitor;
use crate::shared::{BackendMessage, FrontendMessage};
use log::{error, info};

pub async fn run_backend(monitor_only: bool) -> Result<(), Box<dyn std::error::Error>> {
    let xdg_runtime_dir = std::env::var("XDG_RUNTIME_DIR")?;
    let socket_dir = format!("{xdg_runtime_dir}/cursor-clip");
    std::fs::create_dir_all(&socket_dir)?;

    // Only one daemon may own the socket and the clipboard store; a second one
    // would wipe or garbage-collect the first one's data.
    let _instance_lock = acquire_instance_lock(&socket_dir)?;

    // Remove existing socket if it exists
    let socket_path = format!("{socket_dir}/cursor-clip.sock");
    let _ = std::fs::remove_file(&socket_path);

    // Create Unix socket for IPC
    let listener = UnixListener::bind(&socket_path)?;
    info!("Clipboard backend listening on {socket_path}");

    let state = Arc::new(Mutex::new(BackendState::new(monitor_only)));
    tokio::spawn(expire_passwords(state.clone()));

    // Start Wayland clipboard monitoring in a separate task
    let wayland_state = state.clone();
    tokio::spawn(async move {
        let monitor = WaylandClipboardMonitor::new(wayland_state);
        if let Err(e) = monitor.start_monitoring() {
            error!("Wayland clipboard monitoring error: {e}");
        }
    });

    // Add some sample data only in debug builds (helps during development without polluting release)
    #[cfg(debug_assertions)]
    {
        let mut state_lock = state.lock().unwrap();
        for sample in [
            "Hello, world Cursor-Clip!",
            "https://github.com/rust-lang/rust",
            "Sample clipboard content for testing the clipboard manager",
            "impl Display for MyStruct {\n    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {\n        write!(f, \"MyStruct\")\n    }\n}",
            "Password4234!Cursor-Clip",
        ] {
            let _ = state_lock.add_clipboard_item_from_text(sample);
        }
    }

    // Handle IPC connections
    loop {
        let (stream, _addr) = listener.accept().await?;
        let state_clone = state.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, state_clone).await {
                error!("Client error: {e}");
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
                let mut state = state.lock().unwrap();
                BackendMessage::History {
                    items: state.get_history(),
                }
            }
            FrontendMessage::SetClipboardById { id, instant_paste } => {
                let mut state = state.lock().unwrap();
                match state.set_clipboard_by_id(id, instant_paste) {
                    Ok(()) => BackendMessage::ClipboardSet,
                    Err(e) => BackendMessage::Error { message: e },
                }
            }
            FrontendMessage::SetPinned { id, pinned } => {
                let mut state = state.lock().unwrap();
                match state.set_pinned(id, pinned) {
                    Ok(()) => BackendMessage::ItemPinned { id, pinned },
                    Err(e) => BackendMessage::Error { message: e },
                }
            }
            FrontendMessage::ClearHistory => {
                let mut state = state.lock().unwrap();
                state.clear_history();
                BackendMessage::HistoryCleared
            }
            FrontendMessage::GetHistoryPersistence => BackendMessage::PersistenceState {
                enabled: state.lock().unwrap().persistence_enabled,
            },
            FrontendMessage::SetHistoryPersistence { enabled } => {
                let mut state = state.lock().unwrap();
                match state.set_persistence_enabled(enabled) {
                    Ok(()) => BackendMessage::PersistenceState {
                        enabled: state.persistence_enabled,
                    },
                    Err(e) => BackendMessage::Error { message: e },
                }
            }
            FrontendMessage::DeleteItemById { id } => {
                let mut state = state.lock().unwrap();
                match state.delete_item_by_id(id) {
                    Ok(()) => BackendMessage::ItemDeleted { id },
                    Err(e) => BackendMessage::Error { message: e },
                }
            }
        };

        let response_json = serde_json::to_string(&response)?;
        writer.write_all(response_json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
    }

    Ok(())
}

/// Sleep until the next secret expires, waking early when history changes.
pub(super) async fn expire_passwords(state: Arc<Mutex<BackendState>>) {
    let notify = state.lock().unwrap().expiry_notify.clone();
    loop {
        let now = unix_timestamp();
        let next_expiry = {
            let mut state = state.lock().unwrap();
            state.expire_password_items(now);
            state.next_password_expiry()
        };
        if let Some(deadline) = next_expiry {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(deadline.saturating_sub(now))) => {},
                _ = notify.notified() => {},
            }
        } else {
            notify.notified().await;
        }
    }
}

fn acquire_instance_lock(socket_dir: &str) -> Result<std::fs::File, Box<dyn std::error::Error>> {
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(format!("{socket_dir}/daemon.lock"))?;
    match lock_file.try_lock() {
        Ok(()) => Ok(lock_file),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err("another cursor-clip daemon is already running".into())
        }
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ipc_client::FrontendClient;
    use crate::shared::config::{UserConfig, config_path};

    #[test]
    fn lost_acknowledgement_preserves_history_on_reopen_and_restart() {
        crate::shared::isolated_test(|| {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let dir = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
                    .join("cursor-clip");
                std::fs::create_dir(&dir).unwrap();
                let path = dir.join("cursor-clip.sock");
                let listener = UnixListener::bind(&path).unwrap();
                let key = Some("isolated test key".to_owned());
                let mut backend = BackendState::with_password(false, key.clone());
                backend.add_clipboard_item_from_text("keep this entry");
                let state = Arc::new(Mutex::new(backend));

                let mut client = UnixStream::connect(&path).await.unwrap();
                client
                    .write_all(b"{\"SetHistoryPersistence\":{\"enabled\":true}}\n")
                    .await
                    .unwrap();
                drop(client);
                let (stream, _) = listener.accept().await.unwrap();
                // The frontend has already disconnected when the daemon sends the ACK.
                let _ = handle_client(stream, state.clone()).await;
                assert!(
                    UserConfig::read(&config_path())
                        .unwrap()
                        .persistence_enabled
                );
                UserConfig::update(&config_path(), |config| config.show_pin = false).unwrap();

                let server = tokio::spawn(async move {
                    for _ in 0..2 {
                        let (stream, _) = listener.accept().await.unwrap();
                        handle_client(stream, state.clone()).await.unwrap();
                    }
                });
                assert!(FrontendClient::persistence(None).await.unwrap());
                assert_eq!(
                    FrontendClient::history().await.unwrap()[0].content_preview,
                    "keep this entry"
                );
                server.await.unwrap();
                let mut restarted = BackendState::with_password(false, key);
                assert!(restarted.persistence_enabled);
                assert_eq!(
                    restarted.get_history()[0].content_preview,
                    "keep this entry"
                );
                restarted.set_persistence_enabled(false).unwrap();
                assert!(
                    !UserConfig::read(&config_path())
                        .unwrap()
                        .persistence_enabled
                );
                assert!(!UserConfig::read(&config_path()).unwrap().show_pin);
            });
        });
    }
}
