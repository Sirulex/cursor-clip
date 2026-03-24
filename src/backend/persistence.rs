use crate::shared::ClipboardItem;
use log::warn;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use stoolap::Database;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BackendConfig {
    persistence_enabled: bool,
}

pub fn load_persistent_history_state_from_config() -> bool {
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

pub struct ClipboardPersistence {
    db: Database,
}

impl std::fmt::Debug for ClipboardPersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardPersistence").finish_non_exhaustive()
    }
}

impl ClipboardPersistence {
    pub fn open_default() -> Result<Self, String> {
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

        Ok(Self { db })
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
            let item_json = row
                .get::<String>(0)
                .map_err(|e| format!("Failed to parse persisted row payload: {e}"))?;
            let item = serde_json::from_str::<ClipboardItem>(&item_json)
                .map_err(|e| format!("Failed to deserialize persisted clipboard item: {e}"))?;
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
            let item_id = u64_to_i64(item.item_id)?;
            let created_ts = u64_to_i64(item.timestamp)?;

            self.db
                .execute(
                    "INSERT INTO clipboard_history (item_id, item_json, created_ts, pinned) VALUES ($1, $2, $3, $4)",
                    (item_id, item_json, created_ts, item.pinned),
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

pub fn warn_persistence_sync_error(context: &str, err: &str) {
    warn!("Persistence {context} failed: {err}");
}
