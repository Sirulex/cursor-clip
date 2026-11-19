use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsFd;
use std::thread::sleep;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

const PASTE_KEYMAP: &[u8] = b"xkb_keymap {\n\
xkb_keycodes \"(unnamed)\" {\n\
minimum = 8;\n\
maximum = 11;\n\
<K1> = 9;\n\
<K2> = 10;\n\
};\n\
xkb_types \"(unnamed)\" { include \"complete\" };\n\
xkb_compatibility \"(unnamed)\" { include \"complete\" };\n\
xkb_symbols \"(unnamed)\" {\n\
key <K1> {[Control_L]};\n\
key <K2> {[v, V]};\n\
};\n\
};\n\0";

/// Evdev codes of the main typing block (digits, letters, punctuation, space).
/// Other codes (media, volume, print, ...) may be grabbed by compositor shortcuts.
/// Texts with more distinct characters are typed with several keymaps.
const TYPING_KEYS: [u32; 48] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 30, 31,
    32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 57,
];

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

struct VirtualKeyboard {
    connection: Connection,
    event_queue: EventQueue<VirtualKeyboardState>,
    keyboard: ZwpVirtualKeyboardV1,
}

impl VirtualKeyboard {
    fn new() -> Result<Self, String> {
        let connection =
            Connection::connect_to_env().map_err(|e| format!("Wayland connection failed: {e}"))?;
        let (globals, event_queue) =
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

        Ok(Self {
            connection,
            event_queue,
            keyboard,
        })
    }

    fn set_keymap(&mut self, keymap: &[u8]) -> Result<(), String> {
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
        let _ = std::fs::remove_file(&path);

        keymap_file
            .write_all(keymap)
            .map_err(|e| format!("Failed to write keymap: {e}"))?;
        keymap_file
            .flush()
            .map_err(|e| format!("Failed to flush keymap: {e}"))?;

        self.keyboard
            .keymap(1, keymap_file.as_fd(), keymap.len() as u32);

        self.event_queue
            .roundtrip(&mut VirtualKeyboardState)
            .map_err(|e| format!("Wayland roundtrip failed: {e}"))?;
        Ok(())
    }

    fn flush(&self) -> Result<(), String> {
        self.connection
            .flush()
            .map_err(|e| format!("Failed to flush virtual keyboard: {e}"))
    }
}

impl Drop for VirtualKeyboard {
    fn drop(&mut self) {
        self.keyboard.destroy();
        let _ = self.connection.flush();
    }
}

pub fn paste_via_virtual_keyboard_shortcut() -> Result<(), String> {
    let mut vk = VirtualKeyboard::new()?;
    vk.set_keymap(PASTE_KEYMAP)?;

    // Press Ctrl, declare modifiers, tap V, then clear modifiers and release Ctrl.
    // Some clients only honor Ctrl combinations when modifier state is sent explicitly.
    vk.keyboard.key(0, 1, 1);
    vk.keyboard.modifiers(4, 0, 0, 0);
    vk.flush()?;
    sleep(Duration::from_millis(10));

    vk.keyboard.key(0, 2, 1);
    vk.flush()?;
    sleep(Duration::from_millis(6));

    vk.keyboard.key(0, 2, 0);
    vk.flush()?;
    sleep(Duration::from_millis(6));

    vk.keyboard.modifiers(0, 0, 0, 0);
    vk.keyboard.key(0, 1, 0);
    vk.flush()
}

fn keysym_name(c: char) -> String {
    match c {
        '\n' => "Return".to_string(),
        '\t' => "Tab".to_string(),
        _ => format!("U{:04X}", c as u32),
    }
}

/// Keymap where evdev key `TYPING_KEYS[i]` produces `chars[i]`.
fn typing_keymap(chars: &[char]) -> Vec<u8> {
    let mut keycodes = String::new();
    let mut symbols = String::new();
    for (i, (c, key)) in chars.iter().zip(TYPING_KEYS).enumerate() {
        keycodes.push_str(&format!("<K{i}> = {};\n", key + 8));
        symbols.push_str(&format!("key <K{i}> {{[{}]}};\n", keysym_name(*c)));
    }
    let mut keymap = format!(
        "xkb_keymap {{\n\
xkb_keycodes \"(unnamed)\" {{\n\
minimum = 8;\n\
maximum = {};\n\
{keycodes}}};\n\
xkb_types \"(unnamed)\" {{ include \"complete\" }};\n\
xkb_compatibility \"(unnamed)\" {{ include \"complete\" }};\n\
xkb_symbols \"(unnamed)\" {{\n\
{symbols}}};\n\
}};\n",
        TYPING_KEYS[TYPING_KEYS.len() - 1] + 8
    )
    .into_bytes();
    keymap.push(0);
    keymap
}

/// Type `text` into the focused window key by key, without going through the clipboard.
pub fn type_text_via_virtual_keyboard(text: &str) -> Result<(), String> {
    let chars: Vec<char> = text.chars().filter(|c| *c != '\r').collect();
    if chars.is_empty() {
        return Ok(());
    }
    let mut vk = VirtualKeyboard::new()?;

    let mut pos = 0;
    while pos < chars.len() {
        // Longest run of text whose distinct characters fit into one keymap.
        let mut keys: Vec<char> = Vec::new();
        let mut end = pos;
        while end < chars.len() {
            if !keys.contains(&chars[end]) {
                if keys.len() == TYPING_KEYS.len() {
                    break;
                }
                keys.push(chars[end]);
            }
            end += 1;
        }

        vk.set_keymap(&typing_keymap(&keys))?;
        for c in &chars[pos..end] {
            let key = TYPING_KEYS[keys.iter().position(|k| k == c).unwrap_or_default()];
            vk.keyboard.key(0, key, 1);
            vk.keyboard.key(0, key, 0);
            vk.flush()?;
            sleep(Duration::from_millis(2));
        }
        pos = end;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_keymap_maps_each_char_to_its_own_key() {
        let keymap = typing_keymap(&['a', '\n', 'é']);
        let keymap = std::str::from_utf8(&keymap[..keymap.len() - 1]).unwrap();
        assert!(keymap.contains("maximum = 65;"));
        assert!(keymap.contains("<K0> = 10;"));
        assert!(keymap.contains("<K2> = 12;"));
        assert!(keymap.contains("key <K0> {[U0061]};"));
        assert!(keymap.contains("key <K1> {[Return]};"));
        assert!(keymap.contains("key <K2> {[U00E9]};"));
    }
}
