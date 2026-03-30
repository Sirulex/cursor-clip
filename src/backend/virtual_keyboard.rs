use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsFd;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

fn keysym_name_for_char(ch: char) -> String {
    match ch {
        '\n' => "Return".to_string(),
        '\t' => "Tab".to_string(),
        '\x1b' => "Escape".to_string(),
        _ => format!("U{:04X}", ch as u32),
    }
}

fn build_keymap_and_sequence(text: &str) -> (Vec<u8>, Vec<u32>) {
    let mut char_to_key: BTreeMap<char, u32> = BTreeMap::new();
    let mut ordered_chars: Vec<char> = Vec::new();

    for ch in text.chars() {
        if !char_to_key.contains_key(&ch) {
            let key_code = (ordered_chars.len() as u32) + 1;
            char_to_key.insert(ch, key_code);
            ordered_chars.push(ch);
        }
    }

    let sequence = text
        .chars()
        .filter_map(|ch| char_to_key.get(&ch).copied())
        .collect::<Vec<_>>();

    let mut keymap = String::new();
    keymap.push_str("xkb_keymap {\n");
    keymap.push_str("xkb_keycodes \"(unnamed)\" {\n");
    keymap.push_str("minimum = 8;\n");
    keymap.push_str(&format!("maximum = {};\n", ordered_chars.len() + 9));
    for i in 0..ordered_chars.len() {
        keymap.push_str(&format!("<K{}> = {};\n", i + 1, i + 9));
    }
    keymap.push_str("};\n");
    keymap.push_str("xkb_types \"(unnamed)\" { include \"complete\" };\n");
    keymap.push_str("xkb_compatibility \"(unnamed)\" { include \"complete\" };\n");
    keymap.push_str("xkb_symbols \"(unnamed)\" {\n");
    for (idx, ch) in ordered_chars.iter().enumerate() {
        keymap.push_str(&format!(
            "key <K{}> {{[{}]}};\n",
            idx + 1,
            keysym_name_for_char(*ch)
        ));
    }
    keymap.push_str("};\n");
    keymap.push_str("};\n");

    let mut bytes = keymap.into_bytes();
    bytes.push(0);
    (bytes, sequence)
}

pub fn type_text_via_virtual_keyboard(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }

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
    let (keymap_bytes, key_sequence) = build_keymap_and_sequence(text);

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

    for key_code in key_sequence {
        keyboard.key(0, key_code, 1);
        connection
            .flush()
            .map_err(|e| format!("Failed to flush key press: {e}"))?;
        std::thread::sleep(Duration::from_millis(2));

        keyboard.key(0, key_code, 0);
        connection
            .flush()
            .map_err(|e| format!("Failed to flush key release: {e}"))?;
        std::thread::sleep(Duration::from_millis(2));
    }

    keyboard.destroy();
    let _ = std::fs::remove_file(path);
    let _ = connection.flush();

    Ok(())
}
