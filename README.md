# Cursor Clip - GTK4 Clipboard Manager with Dynamic Positioning

A modern Wayland clipboard manager built with **Rust**, **GTK4**, **Libadwaita**, and **Wayland Layer Shell** that makes clipboard handling more reliable.
Features a Windows 11–style clipboard history interface with native GNOME design, which is always positioned at the current mouse pointer location.

## Features

<img src="https://github.com/user-attachments/assets/604896e7-b48e-4851-a9f4-1f06f32ab9c2" width="400" alt="Overlay Preview" align="right" />

<div style="margin-right: 400px;">

### 📋 **Windows 11-Style Clipboard History**
- **Clean list interface**: Similar to Windows 11 clipboard history
- **Content type indicators**: Icons for text, URLs, code, files, etc.
- **Rich previews**: Formatted content display with truncation
- **Timestamps**: When each item was copied
- **Quick selection**: Click any item to copy it back to the clipboard

### 🖱️ **Advanced Wayland Integration**
- **Layer Shell Protocol**: Proper overlay positioning above all windows
- **Precise Cursor Tracking**: Real-time mouse position detection
- **Multi-output Support**: Works across multiple monitors

### 🎨 **Native GNOME Design**
- **Libadwaita styling**: Follows GNOME Human Interface Guidelines
- **Native widgets**: HeaderBar, ListBox, ScrolledWindow

### 📂 **Automatic Clipboard Monitoring (Wayland)**
- Stores the last 100 copied items and removes duplicates.
- Automatic classification of content types:
  - 📝 Text
  - 🔗 URLs
  - 💻 Code
  - 🔒 Passwords
  - 📁 File paths
  - 🖼️ Images

</div>

### 🎥 **Video Showcase**
<details>
   <summary><strong>(click to expand)</strong></summary>

   <br>
   <video src="https://github.com/user-attachments/assets/387c6441-fa6f-4d63-bea8-96d0eece85ee" >
      Your browser does not support the video tag. You can watch it here:
      <a href="https://github.com/user-attachments/assets/387c6441-fa6f-4d63-bea8-96d0eece85ee">Video link</a>.
   </video>
</details>

## Compositor Support
   - The backend uses `zwlr_data_control_manager_v1` to automatically monitor and set clipboard content.
   - The frontend uses `zwlr_layer_shell_v1` to retrieve pointer coordinates and show the overlay.
   - Supported compositors (must support both protocols):
     - KDE Plasma (Wayland session)
     - Hyprland
     - Sway
     - niri
     - Labwc
     - Other wlroots-based compositors

### System Requirements
- **Wayland compositor**, **GTK4**, **gtk4-layer-shell**, **libadwaita**, **Rust**

## Building

### Install Dependencies

#### Arch Linux:
```bash
sudo pacman -S gtk4 libadwaita gtk4-layer-shell
```

#### Ubuntu/Debian:
```bash
sudo apt update
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev gtk4-layer-shell
```

#### Fedora:
```bash
sudo dnf install gtk4-devel libadwaita-devel gtk4-layer-shell
```


### Download and Compile

```bash
# Clone the repository
git clone https://gitlab.com/Sirulex/cursor-clip
cd cursor-clip

# Build in release mode
cargo build --release
```

## Usage
1. **Start Background Daemon**: `cursor-clip --daemon`
2. **Launch Overlay**: Run `cursor-clip` without any arguments (ideally bind it to a hotkey, e.g., Super+V)
3. **Trigger**: Your mouse position is automatically captured
4. **View History**: The clipboard history window will appear at your cursor position, showing:
   - **Recent clipboard items** with content previews
   - **Content type icons** (text, URL, code, password, file)
   - **Timestamps** showing when items were copied
   - **Quick actions**: Clear All and Close
5. **Interact**: 
   - **Click any item** to copy it back to the clipboard
   - **Scroll** through your clipboard history
   - **Clear All** to remove all history items
   - **Keyboard navigation**: Use arrow keys or J/K to navigate, Enter to select, Esc to close


## Key Components

```
┌─────────────────────────────────────────────────┐
│                 Cursor Clip                     │
├─────────────────────────────────────────────────┤
│  GTK4 + Libadwaita UI Layer                     │
│  ├── Modern styling with CSS                    │
│  ├── Responsive layouts                         │
│  └── Accessibility features                     │
├─────────────────────────────────────────────────┤
│  Wayland Layer Shell Integration                │
│  ├── zwlr_layer_shell_v1 protocol               │
│  ├── Positioning and anchoring                  │
│  └── Overlay layer management                   │
├─────────────────────────────────────────────────┤
│  Clipboard Management                           │
│  ├── Data Control Manager for privileged access │
│  ├── IPC communication via UNIX domain sockets  │
│  └── IndexMap for clipboard history storage     │
└─────────────────────────────────────────────────┘
```

## Dependencies

### Core Libraries
- **GTK4** (0.10): Modern UI toolkit
- **Libadwaita** (0.8): GNOME's design system
- **gtk4-layer-shell** (0.6): Wayland layer shell integration
- **wayland-client** (0.31): Wayland protocol bindings
- **wayland-protocols** (0.32): Extended Wayland protocols
- **wayland-protocols-wlr** (0.3.9): wlroots-specific Wayland protocols
- **Tokio runtime** (1.47): Asynchronous runtime
- **serde** (1.0): Serialization framework
- **indexmap** (2.11): Ordered map for clipboard history
- **env_logger** (0.11): Logging framework
---

**Built with ❤️ using Rust, GTK4, Libadwaita, and Wayland Layer Shell**

## License

This project is licensed under the GNU General Public License v3.0 (GPL-3.0). See `LICENSE` for the full text.
