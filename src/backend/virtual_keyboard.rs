use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsFd;
use std::thread::sleep;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

struct VirtualKeyboardState;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for VirtualKeyboardState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(VirtualKeyboardState: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(VirtualKeyboardState: ignore ZwpVirtualKeyboardV1);
delegate_noop!(VirtualKeyboardState: ignore WlSeat);

fn build_paste_shortcut_keymap() -> Vec<u8> {
    // Two keys are enough for Ctrl+V: Control_L on K1 and v on K2.
    let mut keymap = String::new();
    keymap.push_str("xkb_keymap {\n");
    keymap.push_str("xkb_keycodes \"(unnamed)\" {\n");
    keymap.push_str("minimum = 8;\n");
    keymap.push_str("maximum = 11;\n");
    keymap.push_str("<K1> = 9;\n");
    keymap.push_str("<K2> = 10;\n");
    keymap.push_str("};\n");
    keymap.push_str("xkb_types \"(unnamed)\" { include \"complete\" };\n");
    keymap.push_str("xkb_compatibility \"(unnamed)\" { include \"complete\" };\n");
    keymap.push_str("xkb_symbols \"(unnamed)\" {\n");
    keymap.push_str("key <K1> {[Control_L]};\n");
    keymap.push_str("key <K2> {[v, V]};\n");
    keymap.push_str("};\n");
    keymap.push_str("};\n");

    let mut bytes = keymap.into_bytes();
    bytes.push(0);
    bytes
}

pub fn paste_via_virtual_keyboard_shortcut() -> Result<(), String> {
    let connection =
        Connection::connect_to_env().map_err(|e| format!("Wayland connection failed: {e}"))?;
    let (globals, mut event_queue) =
        registry_queue_init::<VirtualKeyboardState>(&connection).map_err(|e| e.to_string())?;
    let qh = event_queue.handle();

    let seat = globals
        .bind::<WlSeat, _, _>(&qh, 1..=9, ())
        .map_err(|_| "No wl_seat found for virtual keyboard".to_string())?;

    let manager = globals
        .bind::<ZwpVirtualKeyboardManagerV1, _, _>(&qh, 1..=1, ())
        .map_err(|_| {
            "Compositor does not support zwp_virtual_keyboard_manager_v1".to_string()
        })?;

    let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
    let keymap_bytes = build_paste_shortcut_keymap();

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "cursor-clip-keymap-{}-{}.xkb",
        std::process::id(),
        nanos
    ));

    let mut keymap_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .read(true)
        .open(&path)
        .map_err(|e| format!("Failed to create temporary keymap file: {e}"))?;

    keymap_file
        .write_all(&keymap_bytes)
        .map_err(|e| format!("Failed to write keymap: {e}"))?;
    keymap_file
        .flush()
        .map_err(|e| format!("Failed to flush keymap: {e}"))?;

    keyboard.keymap(1, keymap_file.as_fd(), keymap_bytes.len() as u32);

    let mut vk_state = VirtualKeyboardState;
    event_queue
        .roundtrip(&mut vk_state)
        .map_err(|e| format!("Wayland roundtrip failed: {e}"))?;

    // Press Ctrl, declare modifiers, tap V, then clear modifiers and release Ctrl.
    // Some clients only honor Ctrl combinations when modifier state is sent explicitly.
    keyboard.key(0, 1, 1);
    keyboard.modifiers(4, 0, 0, 0);
    connection
        .flush()
        .map_err(|e| format!("Failed to flush Ctrl down: {e}"))?;
    sleep(Duration::from_millis(10));

    keyboard.key(0, 2, 1);
    connection
        .flush()
        .map_err(|e| format!("Failed to flush V down: {e}"))?;
    sleep(Duration::from_millis(6));

    keyboard.key(0, 2, 0);
    connection
        .flush()
        .map_err(|e| format!("Failed to flush V up: {e}"))?;
    sleep(Duration::from_millis(6));

    keyboard.modifiers(0, 0, 0, 0);
    keyboard.key(0, 1, 0);
    connection
        .flush()
        .map_err(|e| format!("Failed to flush virtual keyboard shortcut: {e}"))?;

    keyboard.destroy();
    let _ = std::fs::remove_file(path);
    let _ = connection.flush();

    Ok(())
}
