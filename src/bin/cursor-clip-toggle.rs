//! Lightweight entry point for keybindings.
//!
//! `cursor-clip` links GTK and libadwaita, and resolving their symbols makes
//! every start take tens of milliseconds before `main` even runs. When an
//! overlay is already alive (resident under Hyprland), all a keypress has to
//! do is poke its control socket, so this binary depends on std only and
//! falls back to the full `cursor-clip` when no overlay is listening.

use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;

fn main() {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let socket = PathBuf::from(runtime_dir)
            .join("cursor-clip")
            .join("overlay.sock");
        // The connection itself is the toggle request.
        if UnixStream::connect(socket).is_ok() {
            return;
        }
    }

    // No overlay running: start one, preferring the binary installed next to us.
    let full_binary = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("cursor-clip")))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("cursor-clip"));
    let error = std::process::Command::new(full_binary)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("cursor-clip-toggle: failed to start cursor-clip: {error}");
    std::process::exit(1);
}
