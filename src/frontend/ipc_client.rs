use crate::shared::{BackendMessage, ClipboardItemPreview, FrontendMessage};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

/// Frontend client for communicating with the backend
pub struct FrontendClient {
    stream: UnixStream,
}

/// The backend has confirmed the setting, even if refreshing history failed.
#[derive(Debug)]
pub struct PersistenceUpdate {
    pub enabled: bool,
    pub history: Result<Vec<ClipboardItemPreview>, String>,
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
        let xdg_runtime_dir = std::env::var("XDG_RUNTIME_DIR")?;
        let socket_path = format!("{xdg_runtime_dir}/cursor-clip/cursor-clip.sock");
        let stream = UnixStream::connect(socket_path)?;
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

    /// Wait for the store transition and refresh away from the GTK main loop.
    pub async fn set_persistence_enabled_in_background(
        enabled: bool,
    ) -> Result<PersistenceUpdate, PersistenceUpdateError> {
        Self::update_persistence_with_connector(enabled, || {
            Self::new().map_err(|error| error.to_string())
        })
        .await
    }

    async fn update_persistence_with_connector(
        enabled: bool,
        connect: impl FnOnce() -> Result<Self, String> + Send + 'static,
    ) -> Result<PersistenceUpdate, PersistenceUpdateError> {
        gtk4::gio::spawn_blocking(move || {
            let mut client = connect().map_err(PersistenceUpdateError::Unconfirmed)?;
            let response = client
                .send_message(FrontendMessage::SetHistoryPersistence { enabled })
                .map_err(|error| PersistenceUpdateError::Unconfirmed(error.to_string()))?;

            let enabled = match response {
                BackendMessage::PersistenceState { enabled } => enabled,
                BackendMessage::Error { message } => {
                    return Err(PersistenceUpdateError::Rejected(message));
                }
                _ => {
                    return Err(PersistenceUpdateError::Unconfirmed(
                        "Unexpected response to persistence request".to_string(),
                    ));
                }
            };

            // An acknowledged transition must not become a failed transition
            // merely because the following history refresh failed.
            Ok(PersistenceUpdate {
                enabled,
                history: client.get_history().map_err(|error| error.to_string()),
            })
        })
        .await
        .map_err(|_| {
            PersistenceUpdateError::Unconfirmed(
                "Persistence update worker stopped unexpectedly".to_string(),
            )
        })?
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

#[cfg(test)]
mod tests {
    use super::*;
    use gtk4::glib::{ControlFlow, MainContext, Priority};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn read_request(reader: &mut BufReader<UnixStream>) -> FrontendMessage {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn reply(reader: &mut BufReader<UnixStream>, response: BackendMessage) {
        let response = serde_json::to_string(&response).unwrap();
        writeln!(reader.get_mut(), "{response}").unwrap();
    }

    fn run_reply_script(replies: Vec<&str>) -> Result<PersistenceUpdate, PersistenceUpdateError> {
        let (stream, backend) = UnixStream::pair().unwrap();
        let replies: Vec<String> = replies.into_iter().map(str::to_string).collect();
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(backend);
            assert!(matches!(
                read_request(&mut reader),
                FrontendMessage::SetHistoryPersistence { enabled: true }
            ));
            for (index, response) in replies.into_iter().enumerate() {
                if index > 0 {
                    assert!(matches!(
                        read_request(&mut reader),
                        FrontendMessage::GetHistory
                    ));
                }
                writeln!(reader.get_mut(), "{response}").unwrap();
            }
        });
        let context = MainContext::new();
        let result = context
            .with_thread_default(|| {
                context.block_on(FrontendClient::update_persistence_with_connector(
                    true,
                    move || Ok(FrontendClient { stream }),
                ))
            })
            .unwrap();
        server.join().unwrap();
        result
    }

    #[test]
    fn persistence_wait_keeps_main_context_timer_running() {
        let context = MainContext::new();
        let server_context = context.clone();
        let (stream, backend) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(backend);
            assert!(matches!(
                read_request(&mut reader),
                FrontendMessage::SetHistoryPersistence { enabled: true }
            ));

            // Only install the heartbeat after IPC has started. The reply is
            // withheld until GTK's context runs it, rather than sleeping and
            // inferring responsiveness from elapsed time.
            let (heartbeat, received) = mpsc::channel();
            let timer = gtk4::glib::timeout_source_new(
                Duration::from_millis(1),
                None,
                Priority::DEFAULT,
                move || {
                    let _ = heartbeat.send(());
                    ControlFlow::Break
                },
            );
            timer.attach(Some(&server_context));
            let responsive = received.recv_timeout(Duration::from_secs(2)).is_ok();
            timer.destroy();
            if !responsive {
                reply(
                    &mut reader,
                    BackendMessage::Error {
                        message: "Main-context timer did not run during the request".to_string(),
                    },
                );
                return false;
            }
            reply(
                &mut reader,
                BackendMessage::PersistenceState { enabled: true },
            );
            assert!(matches!(
                read_request(&mut reader),
                FrontendMessage::GetHistory
            ));
            reply(&mut reader, BackendMessage::History { items: vec![] });
            true
        });

        let result = context
            .with_thread_default(|| {
                context.block_on(FrontendClient::update_persistence_with_connector(
                    true,
                    move || Ok(FrontendClient { stream }),
                ))
            })
            .unwrap();
        assert!(
            server.join().unwrap(),
            "The main context was blocked by IPC"
        );
        let update = result.unwrap();
        assert!(update.enabled);
        assert!(update.history.unwrap().is_empty());
    }

    #[test]
    fn persistence_rejection_is_distinct_from_missing_acknowledgement() {
        assert!(matches!(
            run_reply_script(vec![r#"{"Error":{"message":"Cannot open store"}}"#]),
            Err(PersistenceUpdateError::Rejected(message)) if message == "Cannot open store"
        ));
        assert!(matches!(
            run_reply_script(vec![]),
            Err(PersistenceUpdateError::Unconfirmed(_))
        ));
    }

    #[test]
    fn persistence_acknowledgement_survives_history_refresh_failure() {
        for replies in [
            vec![r#"{"PersistenceState":{"enabled":true}}"#],
            vec![
                r#"{"PersistenceState":{"enabled":true}}"#,
                r#"{"Error":{"message":"History temporarily unavailable"}}"#,
            ],
        ] {
            let update = run_reply_script(replies).unwrap();
            assert!(update.enabled);
            assert!(update.history.is_err());
        }
    }

    #[test]
    fn malformed_or_unexpected_persistence_reply_is_unconfirmed() {
        for reply in ["not JSON", r#""HistoryCleared""#] {
            assert!(matches!(
                run_reply_script(vec![reply]),
                Err(PersistenceUpdateError::Unconfirmed(_))
            ));
        }
    }

    #[test]
    fn connector_failure_and_worker_panic_are_unconfirmed() {
        let context = MainContext::new();
        context
            .with_thread_default(|| {
                let unavailable = context.block_on(
                    FrontendClient::update_persistence_with_connector(true, || {
                        Err("Backend unavailable".to_string())
                    }),
                );
                assert!(matches!(
                    unavailable,
                    Err(PersistenceUpdateError::Unconfirmed(message)) if message == "Backend unavailable"
                ));
                let panicked = context.block_on(
                    FrontendClient::update_persistence_with_connector(true, || {
                        panic!("Synthetic persistence worker failure")
                    }),
                );
                assert!(matches!(
                    panicked,
                    Err(PersistenceUpdateError::Unconfirmed(_))
                ));
            })
            .unwrap();
    }
}
