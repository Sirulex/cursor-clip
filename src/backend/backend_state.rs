use crate::backend::persistence::{
    db_has_persisted_items, generate_and_store_db_password, load_persistence_enabled_from_config,
    migrate_legacy_history, read_db_password_from_keyring_once, warn_persistence_sync_error,
};
use crate::backend::store::{ClipboardStore, OpenBlob};
use crate::backend::virtual_keyboard::paste_via_virtual_keyboard_shortcut;
use crate::backend::wayland_clipboard::MutexBackendState; // for QueueHandle type
use fast_image_resize as fir;
use fast_image_resize::images::Image;
use image::{ImageFormat, RgbaImage};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;
use wayland_client::Proxy;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_seat;
use wayland_client::{Connection, QueueHandle};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::ExtDataControlDeviceV1,
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_source_v1::ExtDataControlSourceV1,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
};

use crate::shared::{
    ClipboardContentType, ClipboardItem, ClipboardItemPreview, PASSWORD_HINT_MIME,
};
use bytes::Bytes;
use indexmap::IndexMap;
use log::{debug, info, warn};

#[derive(Debug, Clone)]
pub enum DataControlManager {
    Wlr(ZwlrDataControlManagerV1),
    Ext(ExtDataControlManagerV1),
}

#[derive(Debug, Clone)]
pub enum DataControlDevice {
    Wlr(ZwlrDataControlDeviceV1),
    Ext(ExtDataControlDeviceV1),
}

#[derive(Debug, Clone)]
pub enum DataControlSource {
    Wlr(ZwlrDataControlSourceV1),
    Ext(ExtDataControlSourceV1),
}

impl DataControlManager {
    pub fn destroy(self) {
        match self {
            Self::Wlr(manager) => manager.destroy(),
            Self::Ext(manager) => manager.destroy(),
        }
    }

    pub fn create_data_source(&self, qh: &QueueHandle<MutexBackendState>) -> DataControlSource {
        match self {
            Self::Wlr(manager) => DataControlSource::Wlr(manager.create_data_source(qh, ())),
            Self::Ext(manager) => DataControlSource::Ext(manager.create_data_source(qh, ())),
        }
    }

    pub fn get_data_device(
        &self,
        seat: &wl_seat::WlSeat,
        qh: &QueueHandle<MutexBackendState>,
    ) -> DataControlDevice {
        match self {
            Self::Wlr(manager) => DataControlDevice::Wlr(manager.get_data_device(seat, qh, ())),
            Self::Ext(manager) => DataControlDevice::Ext(manager.get_data_device(seat, qh, ())),
        }
    }
}

impl DataControlDevice {
    pub fn destroy(self) {
        match self {
            Self::Wlr(device) => device.destroy(),
            Self::Ext(device) => device.destroy(),
        }
    }

    pub fn set_selection(&self, source: Option<&DataControlSource>) {
        match (self, source) {
            (Self::Wlr(device), Some(DataControlSource::Wlr(source))) => {
                device.set_selection(Some(source))
            }
            (Self::Wlr(device), None) => device.set_selection(None),
            (Self::Ext(device), Some(DataControlSource::Ext(source))) => {
                device.set_selection(Some(source))
            }
            (Self::Ext(device), None) => device.set_selection(None),
            _ => warn!("Mismatched data control protocol between device and source"),
        }
    }
}

impl DataControlSource {
    pub fn destroy(self) {
        match self {
            Self::Wlr(source) => source.destroy(),
            Self::Ext(source) => source.destroy(),
        }
    }

    pub fn offer(&self, mime_type: String) {
        match self {
            Self::Wlr(source) => source.offer(mime_type),
            Self::Ext(source) => source.offer(mime_type),
        }
    }

    pub fn id(&self) -> ObjectId {
        match self {
            Self::Wlr(source) => source.id(),
            Self::Ext(source) => source.id(),
        }
    }
}

#[derive(Debug)]
pub struct BackendState {
    // Clipboard history and management
    pub history: Vec<ClipboardItem>,
    pub id_for_next_entry: u64,

    // Wayland objects for clipboard operations
    pub data_control_manager: Option<DataControlManager>,
    pub data_control_device: Option<DataControlDevice>,
    pub qh: Option<QueueHandle<MutexBackendState>>,
    pub seat: Option<wl_seat::WlSeat>,
    pub connection: Option<Connection>,

    // Current clipboard data
    // Mapping of offer ObjectId -> list of MIME types provided by that offer
    pub mime_type_offers: HashMap<ObjectId, Vec<String>>,
    // Currently selected offer id (if any)
    pub current_data_offer: Option<ObjectId>,
    pub current_source_object: Option<DataControlSource>,
    pub current_source_entry_id: Option<u64>,
    // When we programmatically set the selection, the compositor will echo it
    // back as a new offer/selection. If we immediately try to read that offer
    // inside the dispatch callback, we deadlock because the Send event for our
    // own ZwlrDataControlSourceV1 cannot be processed until we return to the
    // event loop. This flag suppresses reading the very next selection so we
    // avoid blocking on our own source.
    pub suppress_next_selection_read: bool,
    // If true, we only monitor external selections and DO NOT immediately
    // re-set (take ownership of) the newly received selection.
    // If false (default), after reading an external selection we immediately
    // set it ourselves so it persists even if the source app exits.
    pub monitor_only: bool,
    pub persistence_enabled: bool,
    // Holds every payload on disk; persistent or session-only depending on the setting.
    pub store: ClipboardStore,
    pub db_password: Option<String>,
    pub expiry_notify: Arc<Notify>,
}

impl Default for BackendState {
    fn default() -> Self {
        Self::new(false)
    }
}

impl BackendState {
    pub fn new(monitor_only: bool) -> Self {
        let persistence_enabled = load_persistence_enabled_from_config();
        let db_password = match read_db_password_from_keyring_once() {
            Ok(password) => password,
            Err(e) => {
                warn!("Failed to read DB password from keyring at startup: {e}");
                None
            }
        };

        let store = match ClipboardStore::open_ephemeral() {
            Ok(store) => store,
            Err(e) => panic!("Cannot create clipboard store: {e}"),
        };

        let mut state = Self {
            history: Vec::new(),
            mime_type_offers: HashMap::new(),
            id_for_next_entry: 1,
            data_control_manager: None,
            data_control_device: None,
            seat: None,
            current_data_offer: None,
            current_source_object: None,
            current_source_entry_id: None,
            qh: None,
            suppress_next_selection_read: false,
            connection: None,
            monitor_only,
            persistence_enabled: false,
            store,
            db_password,
            expiry_notify: Arc::new(Notify::new()),
        };

        if let Err(e) = state.set_persistence_enabled(persistence_enabled) {
            warn!("Failed to initialize persistence from config: {e}");
        }

        state
    }

    pub fn add_clipboard_item_from_mime_map(
        &mut self,
        mut mime_content: IndexMap<String, Bytes>,
    ) -> Option<u64> {
        // Metadata-only offers (including password-manager clear updates) aren't entries.
        if !mime_content
            .iter()
            .any(|(mime, data)| mime != PASSWORD_HINT_MIME && !data.is_empty())
        {
            return None;
        }
        let password_hint = mime_content.contains_key(PASSWORD_HINT_MIME);

        // If we have image/png, prefer showing mime_type + bytes and set type to Image
        let (content_preview, content_type, thumbnail) = if password_hint {
            let preview = [
                "text/plain;charset=utf-8",
                "text/plain",
                "UTF8_STRING",
                "TEXT",
                "STRING",
            ]
            .iter()
            .find_map(|mime| mime_content.get(*mime))
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .map(|text| text.chars().take(200).collect())
            .unwrap_or_else(|| "Password".to_string());
            (preview, ClipboardContentType::Password, None)
        } else if let Some(png_bytes) = mime_content.get("image/png") {
            (
                format!("<image/png {} bytes>", png_bytes.len()),
                ClipboardContentType::Image,
                Self::scale_image(png_bytes),
            )
        } else {
            // Otherwise, if we have text/plain;charset=utf-8, show up to first 200 chars and infer type
            let preview: String =
                if let Some(txt_bytes) = mime_content.get("text/plain;charset=utf-8") {
                    match std::str::from_utf8(txt_bytes.as_ref()) {
                        Ok(s) => s.chars().take(200).collect(),
                        Err(_) => format!("<text/plain;charset=utf-8 {} bytes>", txt_bytes.len()),
                    }
                } else {
                    // Fallback: show placeholder using first mime entry
                    let (mime_name, len) = mime_content
                        .iter()
                        .next()
                        .map(|(k, v)| (k.clone(), v.len()))
                        .unwrap();
                    format!("<{mime_name} {len} bytes>")
                };
            let content_type = ClipboardContentType::type_from_preview(&preview);
            (preview, content_type, None)
        };

        let mut mime_blobs = IndexMap::with_capacity(mime_content.len());
        for (mime, data) in mime_content.drain(..) {
            match self.store.put_blob(&data) {
                Ok(id) => {
                    mime_blobs.insert(mime, id);
                }
                Err(e) => {
                    warn!("Failed to store clipboard payload ({mime}): {e}");
                    return None;
                }
            }
        }
        drop(mime_content);

        let mut item = ClipboardItem {
            item_id: self.id_for_next_entry,
            content_type,
            content_preview,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            pinned: false,
            mime_blobs,
            thumbnail,
        };

        // Remove older copies of the same content; a re-copied pinned item stays pinned.
        let primary = item.primary_blob().map(str::to_owned);
        self.history.retain(|existing| {
            let duplicate = existing.primary_blob() == primary.as_deref();
            if duplicate && existing.pinned {
                item.pinned = true;
            }
            // Re-copying without metadata must not strip a secret's expiry.
            if duplicate
                && !password_hint
                && let Some(hint) = existing.mime_blobs.get(PASSWORD_HINT_MIME)
            {
                item.mime_blobs
                    .insert(PASSWORD_HINT_MIME.to_string(), hint.clone());
                item.content_type = ClipboardContentType::Password;
                item.thumbnail = None;
                item.timestamp = existing.timestamp;
            }
            !duplicate
        });
        let insert_index = if item.pinned {
            0
        } else {
            self.history
                .iter()
                .position(|existing| !existing.pinned)
                .unwrap_or(self.history.len())
        };
        self.history.insert(insert_index, item);
        if self.history.len() > MAX_HISTORY_ITEMS {
            self.history.truncate(MAX_HISTORY_ITEMS);
        }

        let new_id = self.id_for_next_entry;
        self.id_for_next_entry += 1;
        self.history_changed();
        release_freed_memory();
        Some(new_id)
    }

    fn scale_image(img_bytes: &Bytes) -> Option<Bytes> {
        let source = image::load_from_memory(img_bytes.as_ref()).ok()?.to_rgba8();
        let (src_width, src_height) = source.dimensions();

        if src_width == 0 || src_height == 0 {
            return None;
        }

        let max_width = 300u32;
        let max_height = 180u32;
        let scale =
            (max_width as f32 / src_width as f32).min(max_height as f32 / src_height as f32);
        let dst_width = ((src_width as f32 * scale).round() as u32).max(1);
        let dst_height = ((src_height as f32 * scale).round() as u32).max(1);

        let src_image = Image::from_vec_u8(
            src_width,
            src_height,
            source.into_raw(),
            fir::PixelType::U8x4,
        )
        .ok()?;

        let mut dst_image = Image::new(dst_width, dst_height, fir::PixelType::U8x4);
        let mut resizer = fir::Resizer::new();
        let options = fir::ResizeOptions::new()
            .resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Bilinear));

        resizer
            .resize(&src_image, &mut dst_image, Some(&options))
            .ok()?;

        let thumbnail =
            RgbaImage::from_raw(dst_image.width(), dst_image.height(), dst_image.into_vec())?;

        let mut buffer = Cursor::new(Vec::new());
        if image::DynamicImage::ImageRgba8(thumbnail)
            .write_to(&mut buffer, ImageFormat::Jpeg)
            .is_ok()
        {
            Some(Bytes::from(buffer.into_inner()))
        } else {
            None
        }
    }

    #[cfg(debug_assertions)]
    pub fn add_clipboard_item_from_text(&mut self, text: &str) -> Option<u64> {
        let mut mime_content = IndexMap::new();
        mime_content.insert(
            "text/plain;charset=utf-8".to_string(),
            Bytes::copy_from_slice(text.as_bytes()),
        );
        self.add_clipboard_item_from_mime_map(mime_content)
    }

    pub fn get_history(&mut self) -> Vec<ClipboardItemPreview> {
        self.expire_password_items(unix_timestamp());
        self.history
            .iter()
            .map(ClipboardItemPreview::from)
            .collect()
    }

    pub fn get_item_by_id(&self, id: u64) -> Option<ClipboardItem> {
        let now = unix_timestamp();
        self.history
            .iter()
            .find(|i| i.item_id == id && !i.is_expired(now))
            .cloned()
    }

    pub fn next_password_expiry(&self) -> Option<u64> {
        self.history
            .iter()
            .filter_map(ClipboardItem::expires_at)
            .min()
    }

    /// Remove expired secrets, regardless of pin state, and release our selection.
    pub fn expire_password_items(&mut self, now: u64) {
        let expired_source = self.current_source_entry_id.is_some_and(|id| {
            self.history
                .iter()
                .any(|item| item.item_id == id && item.is_expired(now))
        });
        let previous_len = self.history.len();
        self.history.retain(|item| !item.is_expired(now));
        if self.history.len() == previous_len {
            return;
        }
        if expired_source {
            if self.current_source_object.is_some()
                && let Some(device) = &self.data_control_device
            {
                device.set_selection(None);
            }
            if let Some(source) = self.current_source_object.take() {
                source.destroy();
            }
            self.current_source_entry_id = None;
            self.suppress_next_selection_read = false;
            if let Some(connection) = &self.connection
                && let Err(error) = connection.flush()
            {
                warn!("Failed to flush expired password selection: {error}");
            }
        }
        self.history_changed();
    }

    pub fn clear_history(&mut self) {
        self.history.clear();

        // If we clear history while owning a selection source, drop it and
        // re-enable selection reads so external copies keep being tracked.
        if let Some(prev) = self.current_source_object.take() {
            prev.destroy();
        }
        self.current_source_entry_id = None;
        self.suppress_next_selection_read = false;

        self.history_changed();
    }

    pub fn delete_item_by_id(&mut self, entry_id: u64) -> Result<(), String> {
        let index = self
            .history
            .iter()
            .position(|item| item.item_id == entry_id)
            .ok_or_else(|| format!("No clipboard item found with ID: {entry_id}"))?;

        self.history.remove(index);

        if self.current_source_entry_id == Some(entry_id) {
            if let Some(prev) = self.current_source_object.take() {
                prev.destroy();
            }
            self.current_source_entry_id = None;
            // We explicitly destroyed our own source, so the expected Cancelled
            // event may no longer arrive to clear this flag.
            self.suppress_next_selection_read = false;
        }

        self.history_changed();

        Ok(())
    }

    pub fn set_clipboard_by_id(
        &mut self,
        entry_id: u64,
        instant_paste: bool,
    ) -> Result<(), String> {
        let item = self
            .get_item_by_id(entry_id)
            .ok_or_else(|| format!("No clipboard item found with ID: {entry_id}"))?;

        info!("Setting clipboard content by ID {entry_id}");

        let (Some(manager), Some(device), Some(qh)) = (
            &self.data_control_manager,
            &self.data_control_device,
            &self.qh,
        ) else {
            return Err("Wayland clipboard objects not available yet".into());
        };

        // Clean up any previously set source that we own
        if let Some(prev) = self.current_source_object.take() {
            prev.destroy();
        }

        let source = manager.create_data_source(qh);
        for mime in item.mime_blobs.keys() {
            source.offer(mime.clone());
        }
        device.set_selection(Some(&source));
        self.current_source_object = Some(source);
        self.current_source_entry_id = Some(entry_id);
        // Prevent reading back our own just-set selection (would deadlock due to event queue handling)
        self.suppress_next_selection_read = true;
        // Flush the Wayland connection so the compositor sees our selection (very important)
        if let Some(conn) = &self.connection
            && let Err(e) = conn.flush()
        {
            warn!("Failed to flush Wayland connection after setting selection: {e}");
        }
        debug!("Created clipboard source and set selection (id {entry_id})");

        if instant_paste {
            info!("Instant paste via virtual keyboard shortcut for ID {entry_id}");
            std::thread::spawn(move || {
                // Give the overlay a brief moment to close so shortcut targets the previous app.
                std::thread::sleep(std::time::Duration::from_millis(150));
                if let Err(e) = paste_via_virtual_keyboard_shortcut() {
                    warn!("Instant paste failed: {e}");
                }
            });
        }

        Ok(())
    }

    pub fn set_pinned(&mut self, entry_id: u64, pinned: bool) -> Result<(), String> {
        let index = self
            .history
            .iter()
            .position(|item| item.item_id == entry_id)
            .ok_or_else(|| format!("No clipboard item found with ID: {entry_id}"))?;

        let mut item = self.history.remove(index);
        item.pinned = pinned;

        let insert_index = if pinned {
            0
        } else {
            self.history
                .iter()
                .position(|existing| !existing.pinned)
                .unwrap_or(self.history.len())
        };

        self.history.insert(insert_index, item);
        self.history_changed();
        Ok(())
    }

    pub fn set_persistence_enabled(&mut self, enabled: bool) -> Result<(), String> {
        // The overlay re-sends the setting on every open; keep that cheap.
        if enabled == self.persistence_enabled && enabled == self.store.is_persistent() {
            return Ok(());
        }

        if enabled {
            if self.db_password.is_none() {
                if db_has_persisted_items()? {
                    return Err(
                        "Persistent DB already contains data but no password was found in keyring. \
                         Refusing to generate a new password because it would make existing encrypted history unreadable."
                            .to_string(),
                    );
                }
                self.db_password = Some(generate_and_store_db_password()?);
            }

            let password = self
                .db_password
                .as_deref()
                .ok_or_else(|| "Database password unavailable".to_string())?;
            let persistent = ClipboardStore::open_persistent(password)?;

            // Always read what is already on disk first. If that fails we
            // return before writing anything, so existing history is never
            // replaced by an index that does not contain it.
            let mut existing = match migrate_legacy_history(password, &persistent)? {
                Some(migrated) => migrated,
                None => persistent.load_index()?,
            };
            prepare_restored_history(&mut existing, unix_timestamp());
            self.expire_password_items(unix_timestamp());
            let session = self.store.transfer_to(&persistent, &self.history)?;
            let merged = merge_histories(session, existing);
            persistent.save_index(&merged)?;
            persistent.collect_garbage(&merged);

            // Swap only after the new index is safely on disk.
            self.id_for_next_entry = self.id_for_next_entry.max(next_item_id(&merged));
            self.history = merged;
            self.replace_store(persistent);
            self.persistence_enabled = true;
        } else {
            // Turning persistence off removes the on-disk history; the current
            // session keeps working from a store that dies with the daemon.
            let ephemeral = ClipboardStore::open_ephemeral()?;
            let history = self.store.transfer_to(&ephemeral, &self.history)?;
            self.history = history;
            self.replace_store(ephemeral);
            self.persistence_enabled = false;
        }

        release_freed_memory();
        self.expiry_notify.notify_one();
        Ok(())
    }

    fn replace_store(&mut self, store: ClipboardStore) {
        let previous = std::mem::replace(&mut self.store, store);
        if let Err(e) = previous.destroy() {
            warn!("Failed to remove previous clipboard store: {e}");
        }
    }

    /// Open the payload of one MIME type of a history item. Reading and
    /// decrypting the opened blob can then happen without the backend lock.
    pub fn open_item_payload(&self, entry_id: u64, mime_type: &str) -> Result<OpenBlob, String> {
        let item = self
            .history
            .iter()
            .find(|item| item.item_id == entry_id)
            .ok_or_else(|| format!("Clipboard item id {entry_id} no longer exists in history"))?;
        if item.is_expired(unix_timestamp()) {
            return Err(format!("Clipboard item id {entry_id} has expired"));
        }
        let blob = item
            .mime_blobs
            .get(mime_type)
            .ok_or_else(|| format!("No data stored for MIME {mime_type} (id {entry_id})"))?;
        self.store.reader().open(blob)
    }

    fn history_changed(&self) {
        self.expiry_notify.notify_one();
        // Blobs may only go away once an index without them is on disk;
        // otherwise a crash would leave the old index pointing at nothing.
        match self.store.save_index(&self.history) {
            Ok(()) => self.store.collect_garbage(&self.history),
            Err(e) => warn_persistence_sync_error("save", &e),
        }
    }
}

const MAX_HISTORY_ITEMS: usize = 100;

pub(super) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn prepare_restored_history(history: &mut Vec<ClipboardItem>, now: u64) {
    history.retain(|item| {
        let hint_only =
            item.mime_blobs.len() == 1 && item.mime_blobs.contains_key(PASSWORD_HINT_MIME);
        !(item.is_expired(now) || hint_only)
    });
    for item in history {
        if item.mime_blobs.contains_key(PASSWORD_HINT_MIME) {
            item.content_type = ClipboardContentType::Password;
            item.thumbnail = None;
        }
    }
}

/// Combine the session history with history found on disk: session items
/// first (they are newer), older copies of the same content dropped, pinned
/// items on top, ids kept unique.
fn merge_histories(
    session: Vec<ClipboardItem>,
    existing: Vec<ClipboardItem>,
) -> Vec<ClipboardItem> {
    let mut merged = session;
    let mut seen_content: HashSet<String> = merged
        .iter()
        .filter_map(|item| item.primary_blob().map(str::to_owned))
        .collect();
    let mut used_ids: HashSet<u64> = merged.iter().map(|item| item.item_id).collect();
    let mut next_id = next_item_id(&merged).max(next_item_id(&existing));

    for mut item in existing {
        if let Some(primary) = item.primary_blob()
            && !seen_content.insert(primary.to_owned())
        {
            if let Some(hint) = item.mime_blobs.get(PASSWORD_HINT_MIME)
                && let Some(duplicate) = merged.iter_mut().find(|candidate| {
                    candidate.primary_blob() == Some(primary)
                        && !candidate.mime_blobs.contains_key(PASSWORD_HINT_MIME)
                })
            {
                duplicate
                    .mime_blobs
                    .insert(PASSWORD_HINT_MIME.to_string(), hint.clone());
                duplicate.content_type = ClipboardContentType::Password;
                duplicate.thumbnail = None;
                duplicate.timestamp = item.timestamp;
            }
            continue;
        }
        if !used_ids.insert(item.item_id) {
            item.item_id = next_id;
            used_ids.insert(next_id);
            next_id += 1;
        }
        merged.push(item);
    }

    merged.sort_by_key(|item| !item.pinned);
    merged.truncate(MAX_HISTORY_ITEMS);
    merged
}

fn next_item_id(history: &[ClipboardItem]) -> u64 {
    history
        .iter()
        .map(|item| item.item_id)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

/// Large clipboard payloads pass through the heap only briefly; hand the freed
/// pages back to the kernel so the daemon's RSS does not stay at its peak.
fn release_freed_memory() {
    // SAFETY: malloc_trim only releases free heap memory and has no preconditions.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn expiry_timer_wakes_for_new_earlier_deadline() {
        use std::sync::Mutex;
        use std::time::Duration;

        let state = Arc::new(Mutex::new(test_state(false)));
        state
            .lock()
            .unwrap()
            .add_clipboard_item_from_mime_map(password_mimes("later"))
            .unwrap();
        let task = tokio::spawn(crate::backend::ipc_server::expire_passwords(state.clone()));
        tokio::task::yield_now().await;
        let early_id = {
            let mut state = state.lock().unwrap();
            let id = state
                .add_clipboard_item_from_mime_map(password_mimes("earlier"))
                .unwrap();
            state
                .history
                .iter_mut()
                .find(|item| item.item_id == id)
                .unwrap()
                .timestamp = unix_timestamp() - 119;
            id
        };
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if !state
                    .lock()
                    .unwrap()
                    .history
                    .iter()
                    .any(|item| item.item_id == early_id)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        task.abort();
        let _ = task.await;
        let state = Arc::try_unwrap(state).unwrap().into_inner().unwrap();
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0].content_preview, "later");
        state.store.destroy().unwrap();
        result.unwrap();
    }

    fn test_state(persistent: bool) -> BackendState {
        BackendState {
            history: Vec::new(),
            id_for_next_entry: 1,
            mime_type_offers: HashMap::new(),
            data_control_manager: None,
            data_control_device: None,
            seat: None,
            current_data_offer: None,
            current_source_object: None,
            current_source_entry_id: None,
            qh: None,
            suppress_next_selection_read: false,
            connection: None,
            monitor_only: false,
            persistence_enabled: persistent,
            store: ClipboardStore::isolated_test_store(persistent),
            db_password: None,
            expiry_notify: Arc::new(Notify::new()),
        }
    }

    fn password_mimes(text: &str) -> IndexMap<String, Bytes> {
        IndexMap::from([
            (
                "text/plain;charset=utf-8".to_string(),
                Bytes::copy_from_slice(text.as_bytes()),
            ),
            (PASSWORD_HINT_MIME.to_string(), Bytes::new()),
        ])
    }

    #[test]
    fn password_hint_overrides_text_url_code_file_and_image_classification() {
        let mut state = test_state(false);
        for text in [
            "plaintext",
            "https://example.com",
            "fn main() {}",
            "/home/file",
        ] {
            let mut mimes = password_mimes(text);
            mimes.insert("image/png".to_string(), Bytes::from_static(b"not an image"));
            let id = state.add_clipboard_item_from_mime_map(mimes).unwrap();
            let item = state.get_item_by_id(id).unwrap();
            assert_eq!(item.content_type, ClipboardContentType::Password);
            assert_eq!(item.content_preview, text);
            assert!(item.thumbnail.is_none());
            assert_eq!(item.expires_at(), Some(item.timestamp + 120));
        }
        state.store.destroy().unwrap();
    }

    #[test]
    fn hint_only_and_empty_password_updates_do_not_create_history_entries() {
        let mut state = test_state(false);
        let hint_only = IndexMap::from([(
            PASSWORD_HINT_MIME.to_string(),
            Bytes::from_static(b"secret"),
        )]);
        assert_eq!(state.add_clipboard_item_from_mime_map(hint_only), None);
        assert_eq!(
            state.add_clipboard_item_from_mime_map(password_mimes("")),
            None
        );
        assert!(state.history.is_empty());
        state.store.destroy().unwrap();
    }

    #[test]
    fn pinned_password_expires_at_two_minutes_and_removes_persisted_payloads() {
        let mut state = test_state(true);
        let id = state
            .add_clipboard_item_from_mime_map(password_mimes("secret"))
            .unwrap();
        state.set_pinned(id, true).unwrap();
        state.history[0].timestamp = 100;
        let blob = state.history[0].mime_blobs["text/plain;charset=utf-8"].clone();
        state.current_source_entry_id = Some(id);
        state.suppress_next_selection_read = true;
        state.expire_password_items(219);
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.store.get_blob(&blob).unwrap(), b"secret");
        state.expire_password_items(220);
        assert!(state.history.is_empty());
        assert!(state.store.load_index().unwrap().is_empty());
        assert!(state.store.get_blob(&blob).is_err());
        assert_eq!(state.current_source_entry_id, None);
        assert!(!state.suppress_next_selection_read);
        state.store.destroy().unwrap();
    }

    #[test]
    fn expiry_keeps_newer_selection_and_unmarked_passwords() {
        let mut state = test_state(false);
        state
            .add_clipboard_item_from_mime_map(password_mimes("secret"))
            .unwrap();
        state.history[0].timestamp = 100;
        let mut mimes = IndexMap::new();
        mimes.insert(
            "text/plain;charset=utf-8".to_string(),
            Bytes::from_static(b"NotSecret!"),
        );
        let newer_id = state.add_clipboard_item_from_mime_map(mimes).unwrap();
        state.current_source_entry_id = Some(newer_id);
        state.suppress_next_selection_read = true;
        state.expire_password_items(220);
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0].item_id, newer_id);
        assert_eq!(
            state.history[0].content_type,
            ClipboardContentType::Password
        );
        assert!(state.history[0].expires_at().is_none());
        assert_eq!(state.current_source_entry_id, Some(newer_id));
        assert!(state.suppress_next_selection_read);
        state.store.destroy().unwrap();
    }

    #[test]
    fn copying_duplicate_without_hint_cannot_strip_password_expiry() {
        let mut state = test_state(false);
        let id = state
            .add_clipboard_item_from_mime_map(password_mimes("plaintext"))
            .unwrap();
        state.set_pinned(id, true).unwrap();
        state.history[0].timestamp = 100;
        let mimes = IndexMap::from([(
            "text/plain;charset=utf-8".to_string(),
            Bytes::from_static(b"plaintext"),
        )]);
        state.add_clipboard_item_from_mime_map(mimes).unwrap();
        assert_eq!(state.history.len(), 1);
        assert!(state.history[0].pinned);
        assert_eq!(
            state.history[0].content_type,
            ClipboardContentType::Password
        );
        assert_eq!(state.history[0].expires_at(), Some(220));
        state.expire_password_items(220);
        assert!(state.history.is_empty());
        state.store.destroy().unwrap();
    }

    #[test]
    fn expired_password_cannot_be_pasted_or_read_before_timer_runs() {
        let mut state = test_state(false);
        let id = state
            .add_clipboard_item_from_mime_map(password_mimes("secret"))
            .unwrap();
        state.history[0].timestamp = unix_timestamp().saturating_sub(120);
        assert!(state.get_item_by_id(id).is_none());
        assert!(
            state
                .open_item_payload(id, "text/plain;charset=utf-8")
                .is_err()
        );
        assert!(state.set_clipboard_by_id(id, false).is_err());
        assert!(state.get_history().is_empty());
        state.store.destroy().unwrap();
    }

    #[test]
    fn restoring_history_drops_expired_and_hint_only_items_and_corrects_types() {
        let mut expired = item(1, "expired", true);
        expired.timestamp = 100;
        expired
            .mime_blobs
            .insert(PASSWORD_HINT_MIME.to_string(), "hint".to_string());
        let mut fresh = item(2, "fresh", false);
        fresh.timestamp = 200;
        fresh
            .mime_blobs
            .insert(PASSWORD_HINT_MIME.to_string(), "hint".to_string());
        let mut hint_only = item(3, "hint", false);
        hint_only.timestamp = 200;
        hint_only.mime_blobs.clear();
        hint_only
            .mime_blobs
            .insert(PASSWORD_HINT_MIME.to_string(), "hint".to_string());
        let ordinary = item(4, "ordinary", false);
        let mut history = vec![expired, fresh, hint_only, ordinary];
        prepare_restored_history(&mut history, 220);
        assert_eq!(
            history.iter().map(|item| item.item_id).collect::<Vec<_>>(),
            [2, 4]
        );
        assert_eq!(history[0].content_type, ClipboardContentType::Password);
        assert_eq!(
            ClipboardItemPreview::from(&history[0]).expires_at,
            Some(320)
        );
    }

    fn item(id: u64, blob: &str, pinned: bool) -> ClipboardItem {
        let mut mime_blobs = IndexMap::new();
        mime_blobs.insert("text/plain".to_string(), blob.to_string());
        ClipboardItem {
            item_id: id,
            content_preview: blob.to_string(),
            content_type: ClipboardContentType::Text,
            timestamp: id,
            pinned,
            mime_blobs,
            thumbnail: None,
        }
    }

    #[test]
    fn merge_keeps_disk_history_dedupes_and_keeps_ids_unique() {
        let session = vec![item(1, "new", false), item(2, "same", false)];
        let existing = vec![
            item(1, "old", false),
            item(7, "same", false),
            item(8, "pinned", true),
        ];

        let merged = merge_histories(session, existing);
        let blobs: Vec<_> = merged.iter().map(|i| i.content_preview.as_str()).collect();
        assert_eq!(blobs, ["pinned", "new", "same", "old"]);

        let mut ids: Vec<_> = merged.iter().map(|i| i.item_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), merged.len());
        assert!(next_item_id(&merged) > 8);
    }

    #[test]
    fn merge_truncates_to_history_limit() {
        let existing = (1..=150)
            .map(|id| item(id, &format!("b{id}"), false))
            .collect();
        assert_eq!(
            merge_histories(Vec::new(), existing).len(),
            MAX_HISTORY_ITEMS
        );
    }

    #[test]
    fn merge_preserves_password_marker_on_unmarked_session_duplicate() {
        let mut sensitive = item(1, "same", false);
        sensitive.timestamp = 200;
        sensitive
            .mime_blobs
            .insert(PASSWORD_HINT_MIME.to_string(), "hint".to_string());
        let merged = merge_histories(vec![item(2, "same", false)], vec![sensitive]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].item_id, 2);
        assert_eq!(merged[0].content_type, ClipboardContentType::Password);
        assert_eq!(merged[0].expires_at(), Some(320));
    }
}
