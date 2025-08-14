# Cursor Clip - Clipboard History Manager with Libadwaita

A modern Wayland clipboard manager built with **Rust**, **GTK4**, **Libadwaita**, and **Wayland Layer Shell**. Features a Windows 11-style clipboard history interface with native GNOME design.

## Features

### 📋 **Windows 11-Style Clipboard History**
- **Clean list interface**: Similar to Windows 11 clipboard history
- **Content type indicators**: Icons for text, URLs, code, files, etc.
- **Rich previews**: Formatted content display with truncation
- **Time stamps**: When each item was copied
- **Quick selection**: Click any item to copy it back to clipboard

### 🎨 **Native GNOME Design**
- **Libadwaita styling**: Follows GNOME Human Interface Guidelines
- **Adaptive theming**: Automatically follows system light/dark theme
- **Native widgets**: HeaderBar, ListBox, ScrolledWindow
- **Accessibility**: Full keyboard navigation and screen reader support
- **Responsive layout**: Resizable window with proper content scaling

### 🖱️ **Advanced Wayland Integration**
- **Layer Shell Protocol**: Proper overlay positioning above all windows
- **Precise Cursor Tracking**: Real-time mouse position detection
- **Multi-output Support**: Works across multiple monitors
<!-- - **Non-intrusive**: Doesn't steal focus or interfere with other applications -->

### 📂 **Automatic Clipboard Monitoring (Wayland)**
- The backend uses `zwlr_data_control_manager_v1` to automatically monitor clipboard content.
- Supports:
  - Text/Plain
  - Primary selection (mouse selection)
  - Normal clipboard (Ctrl+C)
- Automatic classification of content types:
  - 📝 Text
  - 🔗 URLs
  - 💻 Code
  - 🔒 Passwords
  - 📁 File paths
- Stores the last 100 copied items and removes duplicates.

## Architecture

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
│  Wayland Protocol Handlers                      │
│  ├── Compositor communication                   │
│  ├── Pointer event processing                   │
│  └── Surface lifecycle management               │
└─────────────────────────────────────────────────┘
```

## Dependencies

### Core Libraries
- **GTK4** (0.9): Modern UI toolkit
- **Libadwaita** (0.7): GNOME's design system  
- **gtk4-layer-shell** (0.5): Wayland layer shell integration
- **wayland-client** (0.31): Wayland protocol bindings
- **wayland-protocols** (0.32): Extended Wayland protocols

### System Requirements
- **Wayland compositor** with layer shell support (GNOME, KDE, Sway, etc.)
- **GTK4** and **Libadwaita** system libraries
- **Rust** 1.70+ (2024 edition support)

## Building

### Install Dependencies

#### Ubuntu/Debian:
```bash
sudo apt update
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev gtk4-layer-shell
```

#### Fedora:
```bash
sudo dnf install gtk4-devel libadwaita-devel gtk4-layer-shell
```

#### Arch Linux:
```bash
sudo pacman -S gtk4 libadwaita gtk4-layer-shell
```

### Compile and Run

```bash
# Clone the repository
git clone <repository-url>
cd cursor-clip

# Build in release mode
cargo build --release

# Run the application
cargo run
```

## Usage

1. **Launch**: Run `cursor-clip` in a Wayland session
2. **Trigger**: Your Mouse position is automatically grabbed
3. **View History**: The clipboard history window will appear at your cursor position showing:
   - **Recent clipboard items** with content previews
   - **Content type icons** (text, URL, code, password, file)
   - **Timestamps** showing when items were copied
   - **Quick actions**: Clear All and Close buttons
4. **Interact**: 
   - **Click any item** to copy it back to the clipboard
   - **Scroll** through your clipboard history
   - **Clear All** to remove all history items
   - **Close** to exit the application

## Interface Components

### Clipboard History List
- **Content Preview**: First 100 characters of each clipboard item
- **Type Detection**: Automatic classification of content types
- **Visual Indicators**: Icons and labels for different content types
- **Time Stamps**: Relative time since each item was copied
- **Hover Effects**: Visual feedback on item selection

### Header Bar
- **Title**: "Clipboard History" with native GNOME styling
- **Clear All Button**: Destructive action button to clear history
- **Window Controls**: Standard minimize/maximize/close controls

## Key Components

### GTK Overlay (`src/gtk_overlay.rs`)
- **Libadwaita Integration**: Full AdwApplication setup
- **Modern UI Components**: ActionRows, HeaderBar, PreferencesPage
- **Custom Styling**: CSS with glassmorphism effects
- **Event Handling**: Button clicks, theme changes, window management
- **Thread-safe State**: RefCell-based state management for GTK objects

### Wayland Integration (`src/main.rs`)
- **Protocol Bindings**: Layer shell, virtual pointer, viewporter
- **Event Loop**: Async event processing with proper cleanup
- **Surface Management**: Capture and update layer surfaces
- **Coordinate Tracking**: Real-time mouse position capture

### State Management (`src/state.rs`)
- **Compositor State**: Wayland object lifecycle management
- **Event Coordination**: Synchronization between Wayland and GTK
- **Resource Cleanup**: Proper disposal of graphics resources

## Advanced Features

### Modern CSS Styling
- **Gradient backgrounds** with transparency
- **Box shadows** and border effects  
- **Hover animations** and focus states
- **Responsive typography** scaling
- **Color scheme** adaptation

### Wayland Layer Shell Configuration
- **Layer positioning**: Overlay layer for top-level display
- **Anchoring system**: Precise coordinate-based positioning
- **Exclusive zones**: Non-intrusive overlay behavior
- **Keyboard modes**: On-demand input handling

### Performance Optimizations
- **Efficient rendering**: Minimal redraws and compositing
- **Memory management**: Proper resource cleanup
- **Event batching**: Optimized message processing
- **Background processing**: Non-blocking UI updates

## Development

### Code Structure
```
src/
├── main.rs              # Application entry point & Wayland setup
├── gtk_overlay.rs       # GTK4/Libadwaita UI implementation  
├── state.rs             # Application state management
├── buffer.rs            # Graphics buffer management
└── dispatch/            # Wayland protocol handlers
    ├── mod.rs
    ├── compositor.rs
    ├── layer_shell.rs
    ├── pointer.rs
    └── ...
```

### Building for Development
```bash
# Development build with debug info
cargo build

# Run with debug logging
RUST_LOG=debug cargo run

# Run tests
cargo test

# Check for linting issues
cargo clippy
```

## Contributing

1. **Follow Rust conventions**: Use `rustfmt` and `clippy`
2. **GTK4 best practices**: Proper widget lifecycle and memory management
3. **Wayland compliance**: Respect protocol specifications
4. **Accessibility**: Ensure keyboard navigation and screen reader support

## License

This project is licensed under the MIT License - see the LICENSE file for details.

---

**Built with ❤️ using Rust, GTK4, Libadwaita, and Wayland Layer Shell**
