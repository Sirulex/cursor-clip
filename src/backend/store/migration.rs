//! Transfer history between stores, publishing the new in-memory history only after
//! the destination index and the user's preference have been committed.
use super::{ClipboardStore, INDEX_FILE};
use crate::shared::{ClipboardContentType, ClipboardItem, PASSWORD_HINT_MIME, write_atomic};
use log::warn;
use std::collections::{HashMap, HashSet};

pub(in crate::backend) const MAX_HISTORY_ITEMS: usize = 100;

impl ClipboardStore {
    pub(in crate::backend) fn migrate_to(
        &mut self,
        target: Self,
        history: &mut Vec<ClipboardItem>,
        now: u64,
        commit_preference: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        // Read and authenticate the old index before changing anything on disk.
        let prior_index = if target.is_persistent() {
            target.read_index()?
        } else {
            None
        };
        let original = match &prior_index {
            Some(sealed) => target.decode_index(sealed)?,
            None => Vec::new(),
        };

        // Incomplete entries can be skipped on restore. Rollback must still
        // preserve every blob named in the original on-disk index.
        let original_blobs = original
            .iter()
            .flat_map(|entry| entry.mime_blobs.values().chain(&entry.thumbnail_blob))
            .cloned()
            .collect();
        let mut restored = target.restore_items(original);
        let prepare = || {
            let mut migrated = self.transfer_to(&target, history)?;
            if target.is_persistent() {
                prepare_restored_history(&mut restored, now);
                migrated = merge_histories(migrated, restored);
                target.save_index(&migrated)?;
            }
            commit_preference()?;
            Ok(migrated)
        };

        let migrated = match prepare() {
            Ok(migrated) => migrated,
            Err(error) => {
                if let Some(index) = prior_index {
                    write_atomic(&target.dir.join(INDEX_FILE), &index).map_err(|restore| {
                        format!("{error}. Restoring previous index failed: {restore}")
                    })?;
                    target.remove_unreferenced_blobs(&original_blobs);
                } else {
                    target.destroy().map_err(|cleanup| {
                        format!("{error}. Removing incomplete store failed: {cleanup}")
                    })?;
                }
                return Err(error);
            }
        };

        if target.is_persistent() {
            target.collect_garbage(&migrated);
        }
        let previous = std::mem::replace(self, target);
        *history = migrated;
        if let Err(error) = previous.destroy() {
            warn!("Failed to remove previous clipboard store: {error}");
        }
        Ok(())
    }
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
    let mut by_content: HashMap<String, usize> = merged
        .iter()
        .enumerate()
        .filter_map(|(index, item)| Some((item.primary_blob()?.to_owned(), index)))
        .collect();
    let mut used_ids: HashSet<u64> = merged.iter().map(|item| item.item_id).collect();
    let mut next_id = next_item_id(&merged).max(next_item_id(&existing));

    for mut item in existing {
        if let Some(primary) = item.primary_blob() {
            if let Some(&index) = by_content.get(primary) {
                merged[index].inherit_duplicate_metadata(&item);
                continue;
            }
            by_content.insert(primary.to_owned(), merged.len());
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

pub(in crate::backend) fn next_item_id(history: &[ClipboardItem]) -> u64 {
    history
        .iter()
        .map(|item| item.item_id)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::ClipboardItemPreview;
    use indexmap::IndexMap;

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
    fn rejected_preference_restores_exact_index_and_discards_transferred_blobs() {
        let mut source = ClipboardStore::isolated_test_store(false);
        let session_blob = source.put_blob(b"session data").unwrap();
        let mut history = vec![item(1, &session_blob, false)];

        let target = ClipboardStore::isolated_test_store(true);
        let saved_blob = target.put_blob(b"saved data").unwrap();
        let mut incomplete = item(2, &saved_blob, true);
        let missing_blob = target.blob_id(b"missing data");
        incomplete
            .mime_blobs
            .insert("text/html".into(), missing_blob);
        target.save_index(&[incomplete]).unwrap();
        let saved_path = target.blob_path(&saved_blob);
        let target_dir = target.dir.clone();
        let index_path = target_dir.join(INDEX_FILE);
        let before = std::fs::read(&index_path).unwrap();
        let transferred_path = target.blob_path(&target.blob_id(b"session data"));

        let error = source
            .migrate_to(target, &mut history, 0, || {
                assert!(transferred_path.exists());
                assert_ne!(std::fs::read(&index_path).unwrap(), before);
                Err("preference write failed".into())
            })
            .unwrap_err();
        assert_eq!(error, "preference write failed");
        assert_eq!(std::fs::read(&index_path).unwrap(), before);
        assert!(saved_path.is_file());
        assert!(!transferred_path.exists());
        assert!(!source.is_persistent());
        assert_eq!(
            source
                .get_blob(&history[0].mime_blobs["text/plain"])
                .unwrap(),
            b"session data"
        );
        std::fs::remove_dir_all(target_dir).unwrap();
        source.destroy().unwrap();
    }

    #[test]
    fn failed_disable_leaves_persistent_history_usable_and_can_be_retried() {
        let mut source = ClipboardStore::isolated_test_store(true);
        let blob = source.put_blob(b"saved data").unwrap();
        let mut history = vec![item(7, &blob, true)];
        source.save_index(&history).unwrap();
        let persistent_dir = source.dir.clone();
        let target = ClipboardStore::isolated_test_store(false);
        let session_dir = target.dir.clone();
        assert!(
            source
                .migrate_to(target, &mut history, 0, || Err(
                    "cannot save preference".into()
                ))
                .is_err()
        );
        assert!(!session_dir.exists());
        assert!(source.is_persistent());
        assert_eq!(
            source.load_index().unwrap()[0].mime_blobs["text/plain"],
            blob
        );

        let target = ClipboardStore::open_at(session_dir, [1; 32], [2; 32], false).unwrap();
        source
            .migrate_to(target, &mut history, 0, || Ok(()))
            .unwrap();
        assert!(!source.is_persistent());
        assert!(!persistent_dir.exists());
        assert_eq!(history[0].item_id, 7);
        assert!(history[0].pinned);
        assert_ne!(history[0].mime_blobs["text/plain"], blob);
        assert_eq!(
            source
                .get_blob(&history[0].mime_blobs["text/plain"])
                .unwrap(),
            b"saved data"
        );
        source.destroy().unwrap();
    }

    #[test]
    fn merge_keeps_disk_history_dedupes_and_keeps_ids_unique() {
        let session = vec![item(1, "new", false), item(2, "same", false)];
        let existing = vec![
            item(1, "old", false),
            item(7, "same", true),
            item(8, "pinned", true),
        ];

        let merged = merge_histories(session, existing);
        let blobs: Vec<_> = merged.iter().map(|i| i.content_preview.as_str()).collect();
        assert_eq!(blobs, ["same", "pinned", "new", "old"]);
        assert!(merged[0].pinned);
        assert_eq!(merged[0].item_id, 2);

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
