# Cursor Clip - Wayland Clipboard Manager

A clipboard manager for Wayland compositors with a modern GTK4/libadwaita interface.

## Architecture

The application is now split into two parts:

### Backend (Daemon)
- Runs in the background with `--daemon` flag
- Manages clipboard history in memory (up to 100 items)
- Provides IPC communication via Unix socket (`/tmp/cursor-clip.sock`)
- **Future**: Will integrate with `zwlr_data_control_manager_v1` to monitor clipboard changes

### Frontend (GUI)
- Runs without flags (default)
- Creates a Wayland layer shell surface for mouse coordinate capture
- Spawns a GTK4/libadwaita overlay window at cursor position
- Communicates with backend via IPC to get/set clipboard data

## Usage

### Start the backend daemon:
```bash
cursor-clip --daemon
```

### Launch the frontend (in another terminal):
```bash
cursor-clip
```

## Features

- **Modern UI**: GTK4 + libadwaita with rounded corners and smooth styling
- **Layer Shell Integration**: Uses zwlr-layer-shell-v1 for precise positioning
- **Clipboard History**: Stores up to 100 clipboard items with timestamps
- **Content Type Detection**: Automatically categorizes text, URLs, code, etc.
- **IPC Communication**: Backend and frontend communicate via Unix sockets
- **Async Architecture**: Built with Tokio for efficient async I/O

## Dependencies

- Wayland compositor with layer shell support
- GTK4 + libadwaita
- Rust 2024 edition

## Project Structure

```
src/
├── main.rs              # Command line argument parsing and mode selection
├── backend.rs           # Clipboard backend daemon (IPC server)
├── frontend.rs          # Frontend coordinator (Wayland + GTK)
├── gtk_overlay.rs       # GTK4/libadwaita UI implementation
├── ipc.rs              # IPC message definitions
├── state.rs            # Wayland state management
├── buffer.rs           # Wayland buffer management
└── dispatch/           # Wayland event dispatchers
    ├── mod.rs
    ├── compositor.rs
    ├── layer_shell.rs
    ├── pointer.rs
    └── ...
```

## Current Status

✅ **Completed**:
- Backend daemon with IPC
- Frontend Wayland layer shell setup
- GTK4/libadwaita overlay window
- Command line argument parsing
- Basic clipboard history management
- Async IPC communication

🚧 **In Progress**:
- Wayland clipboard monitoring (zwlr_data_control_manager_v1)
- System clipboard integration

🎯 **Planned**:
- Persistent clipboard history
- Search and filtering
- Keyboard shortcuts
- Configuration file support

## Testing

The current implementation includes sample clipboard data for testing the UI and IPC communication.

To test:
1. Start the daemon: `cursor-clip --daemon`
2. In another terminal, start the frontend: `cursor-clip`
3. The UI should show sample clipboard entries

## Notes

This is a work-in-progress implementation. The Wayland clipboard integration is currently stubbed out to focus on the core architecture and UI. The backend currently serves sample data for testing purposes.
