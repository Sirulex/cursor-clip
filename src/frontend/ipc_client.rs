use crate::shared::{BackendMessage, ClipboardItemPreview, FrontendMessage};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Frontend client for communicating with the backend
pub struct FrontendClient {
    stream: UnixStream,
}

#[derive(Debug)]
pub enum PersistenceUpdateError {
    /// The backend explicitly rejected the request.
    Rejected(String),
    /// No acknowledgement arrived; the backend may still have applied it.
    Unconfirmed(String),
}

impl FrontendClient {
    /// Create a new client
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let stream = UnixStream::connect(Self::socket_path()?)?;
        stream.set_read_timeout(Some(Self::READ_TIMEOUT))?;
        stream.set_write_timeout(Some(Self::READ_TIMEOUT))?;
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

    const READ_TIMEOUT: Duration = Duration::from_secs(5);
    const TRANSITION_TIMEOUT: Duration = Duration::from_secs(30);

    fn socket_path() -> Result<PathBuf, std::env::VarError> {
        Ok(PathBuf::from(std::env::var("XDG_RUNTIME_DIR")?).join("cursor-clip/cursor-clip.sock"))
    }

    // Run socket I/O and history decoding on the application's Tokio runtime.
    // One deadline covers connecting, writing and reading the complete reply.
    async fn request(
        message: FrontendMessage,
        timeout: Duration,
    ) -> Result<BackendMessage, String> {
        let path = Self::socket_path().map_err(|e| e.to_string())?;
        tokio::spawn(async move { Self::request_at(&path, message, timeout).await })
            .await
            .map_err(|e| e.to_string())?
    }

    async fn request_at(
        path: &Path,
        message: FrontendMessage,
        timeout: Duration,
    ) -> Result<BackendMessage, String> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let line = tokio::time::timeout(timeout, async {
            let mut stream = tokio::net::UnixStream::connect(path).await?;
            let mut request = serde_json::to_vec(&message)?;
            request.push(b'\n');
            stream.write_all(&request).await?;
            let mut line = Vec::new();
            tokio::io::BufReader::new(stream)
                .read_until(b'\n', &mut line)
                .await?;
            Ok::<_, std::io::Error>(line)
        })
        .await
        .map_err(|_| "Timed out waiting for clipboard daemon".to_owned())?
        .map_err(|e| e.to_string())?;
        serde_json::from_slice(&line).map_err(|e| e.to_string())
    }

    pub async fn history() -> Result<Vec<ClipboardItemPreview>, String> {
        match Self::request(FrontendMessage::GetHistory, Self::READ_TIMEOUT).await? {
            BackendMessage::History { items } => Ok(items),
            BackendMessage::Error { message } => Err(message),
            _ => Err("Unexpected response to history request".into()),
        }
    }

    /// Set clipboard by ID
    pub fn set_clipboard_by_id(
        &mut self,
        id: u64,
        instant_paste: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let response =
            self.send_message(FrontendMessage::SetClipboardById { id, instant_paste })?;
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
            BackendMessage::ItemPinned {
                id: confirmed_id,
                pinned: confirmed_pin,
            } if confirmed_id == id && confirmed_pin == pinned => Ok(()),
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

    pub async fn persistence(enabled: Option<bool>) -> Result<bool, PersistenceUpdateError> {
        let (message, timeout) = match enabled {
            Some(enabled) => (
                FrontendMessage::SetHistoryPersistence { enabled },
                Self::TRANSITION_TIMEOUT,
            ),
            None => (FrontendMessage::GetHistoryPersistence, Self::READ_TIMEOUT),
        };
        match Self::request(message, timeout)
            .await
            .map_err(PersistenceUpdateError::Unconfirmed)?
        {
            BackendMessage::PersistenceState { enabled } => Ok(enabled),
            BackendMessage::Error { message } => Err(PersistenceUpdateError::Rejected(message)),
            _ => Err(PersistenceUpdateError::Unconfirmed(
                "Unexpected persistence reply".into(),
            )),
        }
    }

    /// Delete a single clipboard item by ID
    pub fn delete_item_by_id(&mut self, id: u64) -> Result<(), Box<dyn std::error::Error>> {
        let response = self.send_message(FrontendMessage::DeleteItemById { id })?;
        match response {
            BackendMessage::ItemDeleted { id: confirmed_id } if confirmed_id == id => Ok(()),
            BackendMessage::Error { message } => Err(message.into()),
            _ => Err("Unexpected response".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn gtk_context_runs_during_requests_and_distinguishes_bad_replies() {
        use gtk4::glib::{ControlFlow, MainContext, Priority};
        use std::os::unix::net::UnixListener;
        use std::sync::mpsc;
        crate::shared::isolated_test(|| {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let _enter = runtime.enter();
            let context = MainContext::new();
            let path = FrontendClient::socket_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let listener = UnixListener::bind(path).unwrap();
            let server_context = context.clone();
            let server = std::thread::spawn(move || {
                for response in [
                    r#"{"PersistenceState":{"enabled":true}}"#,
                    r#"{"Error":{"message":"rejected"}}"#,
                    "invalid JSON",
                    r#""HistoryCleared""#,
                ] {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut line = String::new();
                    BufReader::new(&stream).read_line(&mut line).unwrap();
                    assert!(matches!(
                        serde_json::from_str(&line).unwrap(),
                        FrontendMessage::SetHistoryPersistence { enabled: true }
                    ));
                    let (tx, rx) = mpsc::channel();
                    let heartbeat =
                        gtk4::glib::idle_source_new(None, Priority::DEFAULT, move || {
                            tx.send(()).unwrap();
                            ControlFlow::Break
                        });
                    heartbeat.attach(Some(&server_context));
                    rx.recv_timeout(Duration::from_secs(2))
                        .expect("GTK blocked waiting for the daemon");
                    writeln!(stream, "{response}").unwrap();
                }
            });
            context.block_on(async {
                assert!(FrontendClient::persistence(Some(true)).await.unwrap());
                assert!(matches!(
                    FrontendClient::persistence(Some(true)).await,
                    Err(PersistenceUpdateError::Rejected(_))
                ));
                for _ in 0..2 {
                    assert!(matches!(
                        FrontendClient::persistence(Some(true)).await,
                        Err(PersistenceUpdateError::Unconfirmed(_))
                    ));
                }
            });
            server.join().unwrap();
        });
    }

    #[tokio::test]
    async fn stalled_ack_and_history_reads_release_the_socket() {
        let path = std::env::temp_dir().join(format!("cc-stall-{:x}.sock", rand::random::<u64>()));
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        for message in [
            FrontendMessage::SetHistoryPersistence { enabled: true },
            FrontendMessage::GetHistory,
        ] {
            let server = async {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                stream.read_to_end(&mut request).await.unwrap();
                assert_eq!(request.last(), Some(&b'\n'));
            };
            let client = FrontendClient::request_at(&path, message, Duration::from_millis(50));
            let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
                tokio::join!(client, server)
            })
            .await
            .expect("socket deadline did not release the connection");
            assert!(result.unwrap_err().contains("Timed out"));
        }
        std::fs::remove_file(path).unwrap();
    }
}
