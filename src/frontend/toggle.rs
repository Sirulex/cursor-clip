use std::fs;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

/// Owns the control socket for the currently active overlay.
///
/// If an overlay already owns the socket, [`ToggleServer::acquire`] signals
/// that instance and returns `None` instead of starting another frontend.
pub struct ToggleServer {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl ToggleServer {
    pub fn acquire() -> io::Result<Option<Self>> {
        let xdg_runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e))?;
        let socket_path = PathBuf::from(xdg_runtime_dir)
            .join("cursor-clip")
            .join("overlay.sock");

        Self::acquire_at(socket_path)
    }

    fn acquire_at(socket_path: PathBuf) -> io::Result<Option<Self>> {
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // The first bind can encounter a socket left behind by a crashed
        // frontend. Only remove it after confirming that nobody is listening.
        for _ in 0..2 {
            match UnixListener::bind(&socket_path) {
                Ok(listener) => {
                    listener.set_nonblocking(true)?;
                    return Ok(Some(Self {
                        listener,
                        socket_path,
                    }));
                }
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                    match UnixStream::connect(&socket_path) {
                        Ok(_stream) => return Ok(None),
                        Err(connect_error)
                            if matches!(
                                connect_error.kind(),
                                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                            ) =>
                        {
                            match fs::remove_file(&socket_path) {
                                Ok(()) => continue,
                                Err(remove_error)
                                    if remove_error.kind() == io::ErrorKind::NotFound =>
                                {
                                    continue;
                                }
                                Err(remove_error) => return Err(remove_error),
                            }
                        }
                        Err(connect_error) => return Err(connect_error),
                    }
                }
                Err(error) => return Err(error),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "could not acquire overlay control socket",
        ))
    }

    pub fn try_clone_listener(&self) -> io::Result<UnixListener> {
        self.listener.try_clone()
    }

    pub fn raw_fd(&self) -> RawFd {
        self.listener.as_raw_fd()
    }

    /// Accept all pending connections. Each connection represents one toggle
    /// request; the message body is deliberately irrelevant.
    pub fn take_toggle_request(&self) -> io::Result<bool> {
        let mut requested = false;

        loop {
            match self.listener.accept() {
                Ok((_stream, _address)) => requested = true,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(requested),
                Err(error) => return Err(error),
            }
        }
    }

    pub fn take_toggle_request_from(listener: &UnixListener) -> io::Result<bool> {
        let mut requested = false;

        loop {
            match listener.accept() {
                Ok((_stream, _address)) => requested = true,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(requested),
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for ToggleServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_PATH: AtomicU64 = AtomicU64::new(0);

    fn test_socket_path() -> PathBuf {
        let id = NEXT_TEST_PATH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("cursor-clip-toggle-{}-{id}", std::process::id()))
            .join("overlay.sock")
    }

    #[test]
    fn second_instance_requests_toggle_instead_of_acquiring() {
        let path = test_socket_path();
        let server = ToggleServer::acquire_at(path.clone()).unwrap().unwrap();

        assert!(ToggleServer::acquire_at(path.clone()).unwrap().is_none());
        assert!(ToggleServer::acquire_at(path.clone()).unwrap().is_none());
        assert!(server.take_toggle_request().unwrap());

        drop(server);
        assert!(!path.exists());
        let _ = fs::remove_dir(path.parent().unwrap());
    }

    #[test]
    fn stale_socket_is_replaced() {
        let path = test_socket_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let stale_listener = UnixListener::bind(&path).unwrap();
        drop(stale_listener);

        let server = ToggleServer::acquire_at(path.clone()).unwrap().unwrap();
        drop(server);

        assert!(!path.exists());
        let _ = fs::remove_dir(path.parent().unwrap());
    }
}
