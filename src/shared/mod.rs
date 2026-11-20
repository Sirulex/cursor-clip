pub mod config;
pub mod data_structures;

pub use data_structures::*;

pub fn write_atomic(path: &std::path::Path, contents: &[u8]) -> Result<(), String> {
    use rand::Rng;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
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
