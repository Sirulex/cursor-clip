use crate::backend::store::ClipboardStore;
use crate::shared::{ClipboardContentType, ClipboardItem};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::Bytes;
use indexmap::IndexMap;
use keyring::Entry;
use log::{info, warn};
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use stoolap::Database;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BackendConfig {
    #[serde(alias = "persistent_history")]
    persistence_enabled: bool,
}

pub fn load_persistence_enabled_from_config() -> bool {
    let path = config_path();
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };

    toml::from_str::<BackendConfig>(&contents)
        .map(|cfg| cfg.persistence_enabled)
        .unwrap_or(false)
}

pub fn history_db_path() -> PathBuf {
    config_dir().join("history.stoolap.db")
}

const KEYRING_SERVICE: &str = "cursor-clip";
const KEYRING_DB_USERNAME: &str = "clipboard-db-key";

pub fn read_db_password_from_keyring_once() -> Result<Option<String>, String> {
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_DB_USERNAME)
        .map_err(|e| format!("Failed to create keyring entry: {e}"))?;

    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Failed to read DB password from keyring: {e}")),
    }
}

pub fn generate_and_store_db_password() -> Result<String, String> {
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_DB_USERNAME)
        .map_err(|e| format!("Failed to create keyring entry: {e}"))?;

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let password = BASE64.encode(bytes);

    entry
        .set_password(&password)
        .map_err(|e| format!("Failed to store DB password in keyring: {e}"))?;

    // Verify write-through immediately so we fail early when no secret-service
    // backend is available (instead of silently creating unusable encrypted data).
    let roundtrip = entry
        .get_password()
        .map_err(|e| format!("Failed to verify DB password in keyring: {e}"))?;
    if roundtrip != password {
        return Err("Stored DB password in keyring did not round-trip correctly".to_string());
    }

    Ok(password)
}

/// True if an encrypted history exists that would become unreadable if a new
/// keyring password were generated.
pub fn db_has_persisted_items() -> Result<bool, String> {
    if ClipboardStore::persistent_index_exists() {
        return Ok(true);
    }
    legacy_db_has_items()
}

fn legacy_db_has_items() -> Result<bool, String> {
    let db_path = history_db_path();
    if !db_path.exists() {
        return Ok(false);
    }

    let db = open_legacy_db(&db_path)?;
    let count: i64 = db
        .query_one("SELECT COUNT(*) FROM clipboard_history", ())
        .map_err(|e| format!("Failed to count persisted items: {e}"))?;

    Ok(count > 0)
}

fn open_legacy_db(db_path: &Path) -> Result<Database, String> {
    let dsn = format!("file://{}", db_path.display());
    let db = Database::open(&dsn).map_err(|e| {
        format!(
            "Failed to open Stoolap database at {}: {e}",
            db_path.display()
        )
    })?;

    db.execute(
        "CREATE TABLE IF NOT EXISTS clipboard_history (
            item_id BIGINT PRIMARY KEY,
            item_json TEXT NOT NULL,
            created_ts BIGINT NOT NULL,
            pinned BOOLEAN NOT NULL
        )",
        (),
    )
    .map_err(|e| format!("Failed to initialize persistence schema: {e}"))?;

    Ok(db)
}

/// History item as stored by cursor-clip <= 1.0.2, with payloads inline.
#[derive(Deserialize)]
struct LegacyClipboardItem {
    item_id: u64,
    content_preview: String,
    content_type: ClipboardContentType,
    timestamp: u64,
    #[serde(default)]
    pinned: bool,
    mime_data: IndexMap<String, Bytes>,
    thumbnail: Option<Bytes>,
}

/// Move history from the legacy Stoolap database into `store`, one item at a
/// time so the whole payload set is never held in memory at once. The old
/// database directory is renamed (not deleted) once the new index is written.
pub fn migrate_legacy_history(
    password: &str,
    store: &ClipboardStore,
) -> Result<Option<Vec<ClipboardItem>>, String> {
    let db_path = history_db_path();
    if !db_path.exists() || ClipboardStore::persistent_index_exists() {
        return Ok(None);
    }

    info!("Migrating clipboard history from {}", db_path.display());
    let cipher = derive_cipher(password);
    let mut items = Vec::new();
    {
        let db = open_legacy_db(&db_path)?;
        let rows = db
            .query(
                "SELECT item_json FROM clipboard_history ORDER BY pinned DESC, created_ts DESC, item_id DESC",
                (),
            )
            .map_err(|e| format!("Failed to query persisted history: {e}"))?;

        for row in rows {
            let row = row.map_err(|e| format!("Failed to read persisted row: {e}"))?;
            let stored_payload = row
                .get::<String>(0)
                .map_err(|e| format!("Failed to parse persisted row payload: {e}"))?;

            // encrypted rows must decrypt successfully; plain rows are legacy
            let item_json = if stored_payload.starts_with("enc:v1:") {
                decrypt_payload(&cipher, &stored_payload)?
            } else {
                stored_payload
            };
            let legacy = serde_json::from_str::<LegacyClipboardItem>(&item_json).map_err(|e| {
                format!("Failed to deserialize persisted clipboard item payload: {e}")
            })?;
            drop(item_json);

            let mut mime_blobs = IndexMap::with_capacity(legacy.mime_data.len());
            for (mime, data) in &legacy.mime_data {
                mime_blobs.insert(mime.clone(), store.put_blob(data)?);
            }
            items.push(ClipboardItem {
                item_id: legacy.item_id,
                content_preview: legacy.content_preview,
                content_type: legacy.content_type,
                timestamp: legacy.timestamp,
                pinned: legacy.pinned,
                mime_blobs,
                thumbnail: legacy.thumbnail,
            });
        }
    }

    store.save_index(&items)?;

    let migrated_path = db_path.with_extension("db.migrated");
    if let Err(e) = fs::rename(&db_path, &migrated_path) {
        warn!(
            "Migrated history but could not rename {}: {e}",
            db_path.display()
        );
    } else {
        info!(
            "Migrated {} clipboard items; old database kept at {}",
            items.len(),
            migrated_path.display()
        );
    }

    Ok(Some(items))
}

fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("cursor-clip")
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

fn derive_cipher(password: &str) -> Aes256Gcm {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    let key = hasher.finalize();
    Aes256Gcm::new_from_slice(&key).expect("SHA-256 output must be 32 bytes")
}

fn decrypt_payload(cipher: &Aes256Gcm, payload: &str) -> Result<String, String> {
    let Some(rest) = payload.strip_prefix("enc:v1:") else {
        return Err("Payload is not encrypted".to_string());
    };

    let mut parts = rest.splitn(2, ':');
    let nonce_b64 = parts
        .next()
        .ok_or_else(|| "Missing encrypted nonce".to_string())?;
    let ciphertext_b64 = parts
        .next()
        .ok_or_else(|| "Missing encrypted ciphertext".to_string())?;

    let nonce_bytes = BASE64
        .decode(nonce_b64)
        .map_err(|e| format!("Invalid encrypted nonce: {e}"))?;
    if nonce_bytes.len() != 12 {
        return Err("Invalid encrypted nonce length".to_string());
    }
    let nonce = Nonce::try_from(nonce_bytes.as_slice())
        .map_err(|_| "Invalid encrypted nonce length".to_string())?;

    let ciphertext = BASE64
        .decode(ciphertext_b64)
        .map_err(|e| format!("Invalid encrypted ciphertext: {e}"))?;

    let plaintext = cipher
        .decrypt(&nonce, ciphertext.as_ref())
        .map_err(|e| format!("Decryption failed: {e}"))?;

    String::from_utf8(plaintext).map_err(|e| format!("Invalid UTF-8 plaintext: {e}"))
}

pub fn warn_persistence_sync_error(context: &str, err: &str) {
    warn!("Persistence {context} failed: {err}");
}
