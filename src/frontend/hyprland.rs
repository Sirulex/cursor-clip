//! Fast pointer lookup through Hyprland's IPC socket.
//!
//! The generic path maps a fullscreen capture layer and waits for the
//! compositor to send `wl_pointer.enter`, which can take until the next mouse
//! movement. Hyprland can answer the same question immediately.

use log::debug;
use serde::Deserialize;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct PointerLocation {
    /// Output connector name, e.g. "DP-2"
    pub monitor: String,
    /// Pointer position relative to the monitor, in logical pixels
    pub x: f64,
    pub y: f64,
    /// Logical monitor size
    pub monitor_width: i32,
    pub monitor_height: i32,
}

#[derive(Deserialize)]
struct CursorPos {
    x: f64,
    y: f64,
}

#[derive(Deserialize)]
struct Monitor {
    name: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: f64,
    transform: u32,
    #[serde(default)]
    focused: bool,
}

impl Monitor {
    fn logical_size(&self) -> (f64, f64) {
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        let (width, height) = if self.transform % 2 == 1 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        (width / scale, height / scale)
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        let (width, height) = self.logical_size();
        x >= self.x && x < self.x + width && y >= self.y && y < self.y + height
    }
}

/// Returns `None` when not running under Hyprland or when IPC fails, so the
/// caller can fall back to the protocol-based lookup.
pub fn pointer_location() -> Option<PointerLocation> {
    let socket = socket_path()?;
    let cursor: CursorPos = serde_json::from_slice(&request(&socket, "j/cursorpos")?).ok()?;
    let monitors: Vec<Monitor> = serde_json::from_slice(&request(&socket, "j/monitors")?).ok()?;

    let monitor = monitors
        .iter()
        .find(|monitor| monitor.contains(cursor.x, cursor.y))
        .or_else(|| monitors.iter().find(|monitor| monitor.focused))?;
    let (width, height) = monitor.logical_size();

    let location = PointerLocation {
        monitor: monitor.name.clone(),
        x: cursor.x - monitor.x,
        y: cursor.y - monitor.y,
        monitor_width: width.round() as i32,
        monitor_height: height.round() as i32,
    };
    debug!("Hyprland pointer location: {location:?}");
    Some(location)
}

fn socket_path() -> Option<PathBuf> {
    let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok()?;
    Some(
        PathBuf::from(runtime_dir)
            .join("hypr")
            .join(signature)
            .join(".socket.sock"),
    )
}

fn request(socket: &PathBuf, command: &str) -> Option<Vec<u8>> {
    let mut stream = UnixStream::connect(socket).ok()?;
    let timeout = Some(Duration::from_millis(300));
    stream.set_read_timeout(timeout).ok()?;
    stream.set_write_timeout(timeout).ok()?;
    stream.write_all(command.as_bytes()).ok()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;
    Some(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(x: f64, y: f64, scale: f64, transform: u32) -> Monitor {
        Monitor {
            name: "DP-1".into(),
            x,
            y,
            width: 3840.0,
            height: 2160.0,
            scale,
            transform,
            focused: false,
        }
    }

    #[test]
    fn logical_size_accounts_for_scale_and_rotation() {
        assert_eq!(monitor(0.0, 0.0, 2.0, 0).logical_size(), (1920.0, 1080.0));
        assert_eq!(monitor(0.0, 0.0, 2.0, 1).logical_size(), (1080.0, 1920.0));
    }

    #[test]
    fn contains_uses_logical_layout_coordinates() {
        let m = monitor(4000.0, -1300.0, 2.0, 0);
        assert!(m.contains(4454.0, -582.0));
        assert!(!m.contains(5920.0, -582.0));
        assert!(!m.contains(3999.0, -582.0));
    }
}
