use crate::shared::write_atomic;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UserConfig {
    pub show_trash: bool,
    pub show_pin: bool,
    #[serde(alias = "persistent_history")]
    pub persistence_enabled: bool,
    pub instant_paste: bool,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            show_trash: true,
            show_pin: true,
            persistence_enabled: false,
            instant_paste: true,
        }
    }
}

pub fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("cursor-clip")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

impl UserConfig {
    pub fn read(path: &Path) -> Result<Self, String> {
        match fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).map_err(|e| format!("Invalid {}: {e}", path.display()))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
        }
    }

    pub fn load_or_create(path: &Path) -> Result<Self, String> {
        if !path.try_exists().map_err(|e| e.to_string())? {
            Self::update(path, |_| {})?;
        }
        Self::read(path)
    }

    // Lock a stable sibling because atomic replacement changes the config inode.
    pub fn update(path: &Path, change: impl FnOnce(&mut Self)) -> Result<(), String> {
        let parent = path.parent().ok_or("Missing config directory")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let lock = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        lock.lock().map_err(|e| e.to_string())?;
        let mut config = Self::read(path)?;
        change(&mut config);
        let contents = toml::to_string_pretty(&config).map_err(|e| e.to_string())?;
        write_atomic(path, contents.as_bytes())
    }
}
