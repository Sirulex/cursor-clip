pub mod dispatch;
pub mod frontend_state;
pub mod gtk_overlay;
pub mod hyprland;
pub mod initializer;
pub mod toggle;

pub use initializer::*;

#[derive(Clone, Debug, Default)]
struct OverlayLocation {
    /// Coordinates in logical pixels relative to the selected output
    cursor: (f64, f64),
    monitor_width: i32,
    monitor_height: i32,
    /// Output connector name, or `None` to let the compositor choose
    monitor: Option<String>,
}
