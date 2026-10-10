//! On-disk clipboard store.
//!
//! Clipboard payloads can be large (screenshots, rich text with embedded
//! images), so they are kept out of the daemon's memory. Every payload is
//! written once as an encrypted, content-addressed blob; history items only
//! keep blob ids plus a small preview and thumbnail.
//!
//! A persistent store lives in `$XDG_DATA_HOME/cursor-clip/store` and is keyed
//! by the password from the OS keyring. When persistent history is disabled an
//! ephemeral store with a random in-memory key is used instead, so payloads
//! are still never kept in RAM and become unreadable once the daemon exits.

use crate::shared::{ClipboardContentType, ClipboardItem};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use bytes::Bytes;
use indexmap::IndexMap;
use log::{debug, warn};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const BLOB_MAGIC: &[u8; 4] = b"CCB1";
const NONCE_LEN: usize = 12;
const INDEX_FILE: &str = "index.v1";
const BLOB_DIR: &str = "blobs";

/// Index entry as written to disk. Thumbnails are stored as blobs so the
/// index stays small enough to rewrite on every change.
#[derive(Serialize, Deserialize)]
struct StoredItem {
    item_id: u64,
    content_preview: String,
    content_type: ClipboardContentType,
    timestamp: u64,
    pinned: bool,
    mime_blobs: IndexMap<String, String>,
    thumbnail_blob: Option<String>,
}

pub struct ClipboardStore {
    dir: PathBuf,
    cipher: Aes256Gcm,
    id_key: [u8; 32],
    persistent: bool,
}

impl std::fmt::Debug for ClipboardStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardStore")
            .field("dir", &self.dir)
            .field("persistent", &self.persistent)
            .finish_non_exhaustive()
    }
}

impl ClipboardStore {
    #[cfg(test)]
    pub(super) fn isolated_test_store(persistent: bool) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "cursor-clip-password-test-{}-{}",
            std::process::id(),
            rand::random::<u64>(),
        ));
        Self::open_at(dir, [7u8; 32], [9u8; 32], persistent).unwrap()
    }

    pub fn persistent_dir() -> PathBuf {
        xdg_dir("XDG_DATA_HOME", ".local/share")
            .join("cursor-clip")
            .join("store")
    }

    fn ephemeral_dir() -> PathBuf {
        xdg_dir("XDG_CACHE_HOME", ".cache")
            .join("cursor-clip")
            .join("session")
    }

    /// True if a persistent store with a history index exists on disk.
    pub fn persistent_index_exists() -> bool {
        Self::persistent_dir().join(INDEX_FILE).exists()
    }

    pub fn open_persistent(password: &str) -> Result<Self, String> {
        let mut hasher = Sha256::new();
        hasher.update(b"cursor-clip store key v1");
        hasher.update(password.as_bytes());
        let key: [u8; 32] = hasher.finalize().into();

        let mut hasher = Sha256::new();
        hasher.update(b"cursor-clip blob id v1");
        hasher.update(password.as_bytes());
        let id_key: [u8; 32] = hasher.finalize().into();

        Self::open_at(Self::persistent_dir(), key, id_key, true)
    }

    /// Open a fresh store whose key only exists in memory. Leftovers from a
    /// previous session are unreadable anyway, so they are removed first.
    pub fn open_ephemeral() -> Result<Self, String> {
        let dir = Self::ephemeral_dir();
        remove_dir_if_exists(&dir)?;

        let mut key = [0u8; 32];
        let mut id_key = [0u8; 32];
        rand::rng().fill_bytes(&mut key);
        rand::rng().fill_bytes(&mut id_key);

        Self::open_at(dir, key, id_key, false)
    }

    fn open_at(
        dir: PathBuf,
        key: [u8; 32],
        id_key: [u8; 32],
        persistent: bool,
    ) -> Result<Self, String> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir.join(BLOB_DIR))
            .map_err(|e| format!("Failed to create store directory {}: {e}", dir.display()))?;

        let cipher = Aes256Gcm::new_from_slice(&key).expect("key must be 32 bytes");
        Ok(Self {
            dir,
            cipher,
            id_key,
            persistent,
        })
    }

    pub const fn is_persistent(&self) -> bool {
        self.persistent
    }

    /// Store a payload and return its id. Identical payloads share one blob.
    pub fn put_blob(&self, data: &[u8]) -> Result<String, String> {
        let id = self.blob_id(data);
        let path = self.blob_path(&id);
        if path.exists() {
            return Ok(id);
        }

        let sealed = self.seal(data)?;
        write_atomic(&path, &sealed)?;
        Ok(id)
    }

    pub fn get_blob(&self, id: &str) -> Result<Vec<u8>, String> {
        self.reader().read(id)
    }

    /// Detached handle for reading blobs without holding the backend lock.
    pub fn reader(&self) -> BlobReader {
        BlobReader {
            blob_dir: self.dir.join(BLOB_DIR),
            cipher: self.cipher.clone(),
        }
    }

    /// Persist the history index. No-op for ephemeral stores.
    pub fn save_index(&self, history: &[ClipboardItem]) -> Result<(), String> {
        if !self.persistent {
            return Ok(());
        }

        let mut stored = Vec::with_capacity(history.len());
        for item in history {
            let thumbnail_blob = match &item.thumbnail {
                Some(thumbnail) => Some(self.put_blob(thumbnail)?),
                None => None,
            };
            stored.push(StoredItem {
                item_id: item.item_id,
                content_preview: item.content_preview.clone(),
                content_type: item.content_type,
                timestamp: item.timestamp,
                pinned: item.pinned,
                mime_blobs: item.mime_blobs.clone(),
                thumbnail_blob,
            });
        }

        let json = serde_json::to_vec(&stored)
            .map_err(|e| format!("Failed to serialize history index: {e}"))?;
        let sealed = self.seal(&json)?;
        write_atomic(&self.dir.join(INDEX_FILE), &sealed)
    }

    pub fn load_index(&self) -> Result<Vec<ClipboardItem>, String> {
        let path = self.dir.join(INDEX_FILE);
        let sealed = match fs::read(&path) {
            Ok(sealed) => sealed,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("Failed to read history index: {e}")),
        };
        let json = self.open(&sealed)?;
        let stored: Vec<StoredItem> = serde_json::from_slice(&json)
            .map_err(|e| format!("Failed to parse history index: {e}"))?;

        let mut items = Vec::with_capacity(stored.len());
        for entry in stored {
            let missing = entry
                .mime_blobs
                .values()
                .find(|id| !self.blob_path(id).exists());
            if let Some(id) = missing {
                warn!(
                    "Dropping history item {} with missing blob {id}",
                    entry.item_id
                );
                continue;
            }

            let thumbnail = entry.thumbnail_blob.as_deref().and_then(|id| {
                self.get_blob(id)
                    .map_err(|e| warn!("Failed to load thumbnail for item {}: {e}", entry.item_id))
                    .ok()
                    .map(Bytes::from)
            });

            items.push(ClipboardItem {
                item_id: entry.item_id,
                content_preview: entry.content_preview,
                content_type: entry.content_type,
                timestamp: entry.timestamp,
                pinned: entry.pinned,
                mime_blobs: entry.mime_blobs,
                thumbnail,
            });
        }

        Ok(items)
    }

    /// Copy the blobs referenced by `history` into `target`, returning the
    /// history rewritten to the target's blob ids.
    pub fn transfer_to(
        &self,
        target: &Self,
        history: &[ClipboardItem],
    ) -> Result<Vec<ClipboardItem>, String> {
        let mut transferred = Vec::with_capacity(history.len());
        for item in history {
            let mut mime_blobs = IndexMap::with_capacity(item.mime_blobs.len());
            for (mime, id) in &item.mime_blobs {
                let data = self.get_blob(id)?;
                mime_blobs.insert(mime.clone(), target.put_blob(&data)?);
            }
            transferred.push(ClipboardItem {
                mime_blobs,
                ..item.clone()
            });
        }
        Ok(transferred)
    }

    /// Delete blobs that are no longer referenced by `history`.
    pub fn collect_garbage(&self, history: &[ClipboardItem]) {
        let mut referenced: HashSet<String> = history
            .iter()
            .flat_map(|item| item.mime_blobs.values().cloned())
            .collect();
        if self.persistent {
            referenced.extend(
                history
                    .iter()
                    .filter_map(|item| item.thumbnail.as_deref())
                    .map(|thumbnail| self.blob_id(thumbnail)),
            );
        }

        let Ok(entries) = fs::read_dir(self.dir.join(BLOB_DIR)) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            // Unfinished atomic writes and unknown files are removed as well.
            if !referenced.contains(name) {
                if let Err(e) = fs::remove_file(entry.path()) {
                    warn!("Failed to remove unreferenced blob {name}: {e}");
                } else {
                    debug!("Removed unreferenced blob {name}");
                }
            }
        }
    }

    /// Remove the store and everything in it.
    pub fn destroy(self) -> Result<(), String> {
        remove_dir_if_exists(&self.dir)
    }

    fn blob_id(&self, data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.id_key);
        hasher.update(data);
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn blob_path(&self, id: &str) -> PathBuf {
        self.dir.join(BLOB_DIR).join(id)
    }

    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::try_from(nonce_bytes.as_slice())
            .expect("AES-GCM nonce buffer must be exactly 12 bytes");
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|e| format!("Encryption failed: {e}"))?;

        let mut sealed = Vec::with_capacity(BLOB_MAGIC.len() + NONCE_LEN + ciphertext.len());
        sealed.extend_from_slice(BLOB_MAGIC);
        sealed.extend_from_slice(&nonce_bytes);
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }

    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        open_sealed(&self.cipher, sealed)
    }
}

/// Reads blobs of one store. A blob removed concurrently (garbage collection,
/// store switch) simply fails to read.
#[derive(Clone)]
pub struct BlobReader {
    blob_dir: PathBuf,
    cipher: Aes256Gcm,
}

impl BlobReader {
    pub fn read(&self, id: &str) -> Result<Vec<u8>, String> {
        self.open(id)?.read()
    }

    /// Open a blob now and read it later. The open file stays readable even
    /// if the blob is deleted in between (garbage collection, store switch).
    pub fn open(&self, id: &str) -> Result<OpenBlob, String> {
        if !is_blob_id(id) {
            return Err(format!("Invalid blob id: {id}"));
        }
        let file = fs::File::open(self.blob_dir.join(id))
            .map_err(|e| format!("Failed to open blob {id}: {e}"))?;
        Ok(OpenBlob {
            file,
            cipher: self.cipher.clone(),
            id: id.to_string(),
        })
    }
}

pub struct OpenBlob {
    file: fs::File,
    cipher: Aes256Gcm,
    id: String,
}

impl OpenBlob {
    pub fn read(mut self) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let mut sealed = Vec::new();
        self.file
            .read_to_end(&mut sealed)
            .map_err(|e| format!("Failed to read blob {}: {e}", self.id))?;
        open_sealed(&self.cipher, &sealed)
    }
}

fn open_sealed(cipher: &Aes256Gcm, sealed: &[u8]) -> Result<Vec<u8>, String> {
    let rest = sealed
        .strip_prefix(BLOB_MAGIC.as_slice())
        .ok_or_else(|| "Unknown store file format".to_string())?;
    if rest.len() < NONCE_LEN {
        return Err("Truncated store file".to_string());
    }
    let (nonce_bytes, ciphertext) = rest.split_at(NONCE_LEN);
    let nonce = Nonce::try_from(nonce_bytes).map_err(|_| "Invalid nonce".to_string())?;
    cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|e| format!("Decryption failed: {e}"))
}

fn is_blob_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn xdg_dir(var: &str, home_fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(home_fallback)
        })
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut suffix = [0u8; 8];
    rand::rng().fill_bytes(&mut suffix);
    let tmp_name = format!(
        ".tmp-{}",
        suffix
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let tmp_path = path.with_file_name(tmp_name);

    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path)?;
        file.write_all(contents)?;
        file.sync_data()?;
        fs::rename(&tmp_path, path)?;
        // Make the new name durable before callers delete what it replaces.
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();

    result.map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        format!("Failed to write {}: {e}", path.display())
    })
}

fn remove_dir_if_exists(dir: &Path) -> Result<(), String> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to remove {}: {e}", dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store(persistent: bool) -> ClipboardStore {
        let mut suffix = [0u8; 8];
        rand::rng().fill_bytes(&mut suffix);
        let dir = std::env::temp_dir().join(format!(
            "cursor-clip-store-test-{}-{}",
            std::process::id(),
            suffix
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ));
        ClipboardStore::open_at(dir, [7u8; 32], [9u8; 32], persistent).unwrap()
    }

    fn item(id: u64, blob: &str, thumbnail: Option<&[u8]>) -> ClipboardItem {
        let mut mime_blobs = IndexMap::new();
        mime_blobs.insert("text/plain".to_string(), blob.to_string());
        ClipboardItem {
            item_id: id,
            content_preview: format!("item {id}"),
            content_type: ClipboardContentType::Text,
            timestamp: id,
            pinned: false,
            mime_blobs,
            thumbnail: thumbnail.map(Bytes::copy_from_slice),
        }
    }

    #[test]
    fn blobs_roundtrip_deduplicate_and_are_encrypted() {
        let store = test_store(false);
        let first = store.put_blob(b"secret clipboard text").unwrap();
        let second = store.put_blob(b"secret clipboard text").unwrap();
        assert_eq!(first, second);
        assert_eq!(store.get_blob(&first).unwrap(), b"secret clipboard text");

        let on_disk = fs::read(store.blob_path(&first)).unwrap();
        assert!(!on_disk.windows(6).any(|w| w == b"secret"));
        store.destroy().unwrap();
    }

    #[test]
    fn opened_blob_survives_garbage_collection() {
        let store = test_store(false);
        let id = store.put_blob(b"pasting this").unwrap();
        let opened = store.reader().open(&id).unwrap();

        store.collect_garbage(&[]);
        assert!(!store.blob_path(&id).exists());
        assert_eq!(opened.read().unwrap(), b"pasting this");
        store.destroy().unwrap();
    }

    #[test]
    fn index_roundtrip_and_garbage_collection() {
        let store = test_store(true);
        let kept = store.put_blob(b"kept").unwrap();
        let dropped = store.put_blob(b"dropped").unwrap();
        let history = vec![item(2, &kept, Some(b"thumb")), item(1, &kept, None)];

        store.save_index(&history).unwrap();
        store.collect_garbage(&history);
        assert!(store.blob_path(&kept).exists());
        assert!(!store.blob_path(&dropped).exists());

        let loaded = store.load_index().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].item_id, 2);
        assert_eq!(loaded[0].thumbnail.as_deref(), Some(b"thumb".as_slice()));
        assert_eq!(loaded[1].mime_blobs["text/plain"], kept);
        store.destroy().unwrap();
    }

    #[test]
    fn transfer_rewrites_blob_ids_for_target_store() {
        let source = test_store(false);
        let target = ClipboardStore::open_at(
            source.dir.with_extension("target"),
            [1u8; 32],
            [2u8; 32],
            true,
        )
        .unwrap();
        let blob = source.put_blob(b"payload").unwrap();

        let moved = source
            .transfer_to(&target, &[item(1, &blob, None)])
            .unwrap();
        let new_id = &moved[0].mime_blobs["text/plain"];
        assert_ne!(new_id, &blob);
        assert_eq!(target.get_blob(new_id).unwrap(), b"payload");

        source.destroy().unwrap();
        target.destroy().unwrap();
    }
}
