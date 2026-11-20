use super::store::ClipboardStore;
use keyring::Entry;

const KEYRING_SERVICE: &str = "cursor-clip";
const KEYRING_DB_USERNAME: &str = "clipboard-db-key";

fn keyring_entry() -> Result<Entry, String> {
    Entry::new(KEYRING_SERVICE, KEYRING_DB_USERNAME)
        .map_err(|e| format!("Failed to create keyring entry: {e}"))
}

pub fn read_db_password_from_keyring_once() -> Result<Option<String>, String> {
    let entry = keyring_entry()?;

    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Failed to read DB password from keyring: {e}")),
    }
}

fn generate_and_store_db_password() -> Result<String, String> {
    let entry = keyring_entry()?;

    let password = glib::base64_encode(&rand::random::<[u8; 32]>()).to_string();

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

pub(super) fn open_persistent_store(
    password: &mut Option<String>,
) -> Result<ClipboardStore, String> {
    let password = match password {
        Some(password) => password,
        None if ClipboardStore::persistent_index_exists()? => {
            return Err("Persistent history already exists but no password was found in keyring. \
                        Refusing to generate a new password because it would make existing encrypted history unreadable.".into());
        }
        None => password.insert(generate_and_store_db_password()?),
    };
    ClipboardStore::open_persistent(password)
}
