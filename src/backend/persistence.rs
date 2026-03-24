use crate::shared::ClipboardItem;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use keyring::Entry;
use log::warn;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
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

pub fn db_has_persisted_items() -> Result<bool, String> {
    let db_path = history_db_path();
    if !db_path.exists() {
        return Ok(false);
    }

    let dsn = format!("file://{}", db_path.display());
    let db = Database::open(&dsn)
        .map_err(|e| format!("Failed to open Stoolap database at {}: {e}", db_path.display()))?;

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

    let count: i64 = db
        .query_one("SELECT COUNT(*) FROM clipboard_history", ())
        .map_err(|e| format!("Failed to count persisted items: {e}"))?;

    Ok(count > 0)
}

pub struct ClipboardPersistence {
    db: Database,
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for ClipboardPersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardPersistence").finish_non_exhaustive()
    }
}

impl ClipboardPersistence {
    pub fn open_default(password: &str) -> Result<Self, String> {
        let db_path = history_db_path();
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create persistence directory: {e}"))?;
        }

        let dsn = format!("file://{}", db_path.display());
        let db = Database::open(&dsn)
            .map_err(|e| format!("Failed to open Stoolap database at {}: {e}", db_path.display()))?;

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

        let cipher = derive_cipher(password);

        Ok(Self { db, cipher })
    }

    pub fn load_history(&self) -> Result<Vec<ClipboardItem>, String> {
        let mut items = Vec::new();
        let rows = self
            .db
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

            // Backward-compatible load:
            // - encrypted rows must decrypt successfully
            // - plain rows are accepted for legacy migrations
            let item_json = if stored_payload.starts_with("enc:v1:") {
                decrypt_payload(&self.cipher, &stored_payload)?
            } else {
                stored_payload
            };

            let item = serde_json::from_str::<ClipboardItem>(&item_json).map_err(|e| {
                format!("Failed to deserialize persisted clipboard item payload: {e}")
            })?;
            items.push(item);
        }

        Ok(items)
    }

    pub fn save_history(&self, history: &[ClipboardItem]) -> Result<(), String> {
        self.db
            .execute("DELETE FROM clipboard_history", ())
            .map_err(|e| format!("Failed to clear persisted history: {e}"))?;

        for item in history {
            let item_json = serde_json::to_string(item)
                .map_err(|e| format!("Failed to serialize clipboard item {}: {e}", item.item_id))?;
            let encrypted_payload = encrypt_payload(&self.cipher, &item_json)
                .map_err(|e| format!("Failed to encrypt clipboard item {}: {e}", item.item_id))?;
            let item_id = u64_to_i64(item.item_id)?;
            let created_ts = u64_to_i64(item.timestamp)?;

            self.db
                .execute(
                    "INSERT INTO clipboard_history (item_id, item_json, created_ts, pinned) VALUES ($1, $2, $3, $4)",
                    (item_id, encrypted_payload, created_ts, item.pinned),
                )
                .map_err(|e| format!("Failed to persist clipboard item {}: {e}", item.item_id))?;
        }

        Ok(())
    }
}

fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("cursor-clip")
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

fn u64_to_i64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("Value {value} exceeds i64 range"))
}

fn derive_cipher(password: &str) -> Aes256Gcm {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    let key = hasher.finalize();
    Aes256Gcm::new_from_slice(&key).expect("SHA-256 output must be 32 bytes")
}

fn encrypt_payload(cipher: &Aes256Gcm, plaintext: &str) -> Result<String, String> {
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| format!("Encryption failed: {e}"))?;

    Ok(format!(
        "enc:v1:{}:{}",
        BASE64.encode(nonce_bytes),
        BASE64.encode(ciphertext)
    ))
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
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = BASE64
        .decode(ciphertext_b64)
        .map_err(|e| format!("Invalid encrypted ciphertext: {e}"))?;

    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|e| format!("Decryption failed: {e}"))?;

    String::from_utf8(plaintext).map_err(|e| format!("Invalid UTF-8 plaintext: {e}"))
}

pub fn warn_persistence_sync_error(context: &str, err: &str) {
    warn!("Persistence {context} failed: {err}");
}
