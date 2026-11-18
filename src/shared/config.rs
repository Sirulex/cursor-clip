use crate::shared::write_atomic;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const DEFAULT_LIST_WIDTH: i32 = 350;
pub const DEFAULT_LIST_HEIGHT: i32 = 400;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UserConfig {
    pub show_trash: bool,
    pub show_pin: bool,
    #[serde(alias = "persistent_history")]
    pub persistence_enabled: bool,
    pub instant_paste: bool,
    pub width: i32,
    pub height: i32,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            show_trash: true,
            show_pin: true,
            persistence_enabled: false,
            instant_paste: true,
            width: DEFAULT_LIST_WIDTH,
            height: DEFAULT_LIST_HEIGHT,
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

fn read_config<T: DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("Invalid {}: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

// Invalid display settings must not change which store the daemon opens.
pub fn read_persistence_enabled(path: &Path) -> Result<bool, String> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct StoragePreference {
        #[serde(alias = "persistent_history")]
        persistence_enabled: bool,
    }
    Ok(read_config::<StoragePreference>(path)?.persistence_enabled)
}

impl UserConfig {
    pub fn read(path: &Path) -> Result<Self, String> {
        let config: Self = read_config(path)?;
        if config.height < 1 {
            return Err(format!(
                "Invalid {}: height must be a positive whole number",
                path.display()
            ));
        }
        Ok(config)
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
