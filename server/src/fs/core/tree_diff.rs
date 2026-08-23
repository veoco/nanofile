use std::collections::HashMap;

use crate::fs::core::tree::read_fs_dir_data_batch;
use crate::repository::Repositories;
use base::common::DirEntryData;
use base::error::AppError;
use infra::common::EMPTY_SHA1;

/// A single file-system change detected by diffing two tree snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsChange {
    /// `"create"`, `"delete"`, `"edit"`, `"rename"`, or `"move"`
    pub op_type: &'static str,
    /// `"file"` or `"dir"`
    pub obj_type: &'static str,
    /// Absolute path of the affected entry (e.g. `/docs/intro.md`).
    pub path: String,
    /// File size in bytes (0 for directories).
    pub size: i64,
    /// fs_object ID (SHA1).
    pub obj_id: String,
    /// Previous path for rename/move operations.
    pub old_path: Option<String>,
}

/// Walk an FS tree from `root_fs_id` using a level frontier (no recursion)
/// and populate `out` with every entry's path → (DirEntryData).
/// Directories are included too.
///
/// Each level reads all its directories with one batched `IN` query instead
/// of one query per directory (O(#dirs) → O(depth)).
async fn collect_entries(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
    prefix: &str,
    out: &mut HashMap<String, DirEntryData>,
) -> Result<(), AppError> {
    struct Frame {
        fs_id: String,
        prefix: String,
    }

    let mut frontier: Vec<Frame> = vec![Frame {
        fs_id: root_fs_id.to_string(),
        prefix: prefix.to_string(),
    }];

    while !frontier.is_empty() {
        let ids: Vec<String> = frontier.iter().map(|f| f.fs_id.clone()).collect();
        let dir_map = read_fs_dir_data_batch(repos, repo_id, &ids).await?;
        let mut next: Vec<Frame> = Vec::new();

        for frame in &frontier {
            // Missing/EMPTY dirs are absent from the batch map → skip (same as
            // the per-id `Err(_) => continue` behaviour).
            let Some(dir) = dir_map.get(&frame.fs_id) else {
                continue;
            };
            for entry in &dir.dirents {
                let entry_path = if frame.prefix.is_empty() {
                    format!("/{}", entry.name)
                } else {
                    format!("{}/{}", frame.prefix, entry.name)
                };
                out.insert(entry_path.clone(), entry.clone());

                // Push subdirectories onto the next frontier level.
                if entry.mode & 0o40000 != 0 {
                    next.push(Frame {
                        fs_id: entry.id.clone(),
                        prefix: entry_path,
                    });
                }
            }
        }

        frontier = next;
    }
    Ok(())
}

/// Compare two FS tree snapshots and return the list of changes.
///
/// `old_root_id` should be `None` when there is no previous tree (first
/// commit or empty repo) – in that case every entry in the new tree is
/// reported as `"create"`.
pub async fn diff_trees(
    repos: &Repositories,
    repo_id: &str,
    old_root_id: Option<&str>,
    new_root_id: &str,
) -> Result<Vec<FsChange>, AppError> {
    // If there is no old tree or it is the empty sentinel, everything is new.
    let no_old_tree = old_root_id.is_none() || old_root_id == Some(EMPTY_SHA1);

    if no_old_tree {
        let mut entries = HashMap::new();
        collect_entries(repos, repo_id, new_root_id, "", &mut entries).await?;
        let mut changes: Vec<FsChange> = entries
            .into_iter()
            .map(|(path, entry)| {
                let is_dir = entry.mode & 0o40000 != 0;
                FsChange {
                    op_type: "create",
                    obj_type: if is_dir { "dir" } else { "file" },
                    path,
                    size: entry.size,
                    obj_id: entry.id,
                    old_path: None,
                }
            })
            .collect();
        // Sort by path depth so parents come before children.
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        return Ok(changes);
    }

    let old_root = old_root_id.unwrap();

    // Incremental diff: walk only the subtrees whose object ids differ between
    // old and new, instead of materialising both whole trees. A frame pairs the
    // old and new directory at one path; an absent side means the whole subtree
    // is new (created) or gone (deleted). Delete changes are emitted as soon as
    // an old-only entry is found; renames/moves are matched afterwards by
    // obj_id so the output keeps both the delete and the rename/move, exactly
    // like the full-tree reference diff.
    struct Frame {
        old_fs_id: Option<String>,
        new_fs_id: String,
        prefix: String,
    }

    let mut changes: Vec<FsChange> = Vec::new();
    // New entries pending rename/move matching, then create fallback.
    let mut created: Vec<(String, DirEntryData)> = Vec::new();
    // obj_id → old-side entries removed from their path, for rename/move matching.
    let mut obj_to_deleted: HashMap<String, Vec<(String, DirEntryData)>> = HashMap::new();

    let mut frontier: Vec<Frame> = vec![Frame {
        old_fs_id: Some(old_root.to_string()),
        new_fs_id: new_root_id.to_string(),
        prefix: String::new(),
    }];

    while !frontier.is_empty() {
        let mut old_ids: Vec<String> = Vec::new();
        let mut new_ids: Vec<String> = Vec::new();
        for frame in &frontier {
            new_ids.push(frame.new_fs_id.clone());
            if let Some(o) = &frame.old_fs_id {
                old_ids.push(o.clone());
            }
        }
        let new_map = read_fs_dir_data_batch(repos, repo_id, &new_ids).await?;
        let old_map = if old_ids.is_empty() {
            HashMap::new()
        } else {
            read_fs_dir_data_batch(repos, repo_id, &old_ids).await?
        };

        let mut next: Vec<Frame> = Vec::new();

        for frame in &frontier {
            // A present id that fails to resolve (not a directory / missing)
            // is treated as absent, matching the batch-map `continue` semantics
            // of the full-tree walk.
            let old_dir = frame.old_fs_id.as_ref().and_then(|oid| old_map.get(oid));
            let new_dir = new_map.get(&frame.new_fs_id);

            match (old_dir, new_dir) {
                (None, Some(new_dir)) => {
                    // Old side missing → the whole new subtree is created.
                    for entry in &new_dir.dirents {
                        let path = join_path(&frame.prefix, &entry.name);
                        created.push((path.clone(), entry.clone()));
                        if entry.mode & 0o40000 != 0 {
                            next.push(Frame {
                                old_fs_id: None,
                                new_fs_id: entry.id.clone(),
                                prefix: path,
                            });
                        }
                    }
                }
                (Some(old_dir), None) => {
                    // New side missing → the whole old subtree is deleted.
                    for entry in &old_dir.dirents {
                        let path = join_path(&frame.prefix, &entry.name);
                        changes.push(delete_change(&path, entry));
                        obj_to_deleted
                            .entry(entry.id.clone())
                            .or_default()
                            .push((path.clone(), entry.clone()));
                        if entry.mode & 0o40000 != 0 {
                            next.push(Frame {
                                old_fs_id: Some(entry.id.clone()),
                                new_fs_id: EMPTY_SHA1.to_string(),
                                prefix: path,
                            });
                        }
                    }
                }
                (Some(old_dir), Some(new_dir)) => {
                    let old_entries: HashMap<&str, &DirEntryData> = old_dir
                        .dirents
                        .iter()
                        .map(|d| (d.name.as_str(), d))
                        .collect();

                    for new_entry in &new_dir.dirents {
                        let path = join_path(&frame.prefix, &new_entry.name);
                        let new_is_dir = new_entry.mode & 0o40000 != 0;
                        match old_entries.get(new_entry.name.as_str()) {
                            None => {
                                created.push((path.clone(), new_entry.clone()));
                                if new_is_dir {
                                    next.push(Frame {
                                        old_fs_id: None,
                                        new_fs_id: new_entry.id.clone(),
                                        prefix: path,
                                    });
                                }
                            }
                            Some(old_entry) => {
                                if new_entry.id == old_entry.id {
                                    continue;
                                }
                                if new_is_dir {
                                    // Same path, both directories, different id:
                                    // descend both sides; the entry itself is not
                                    // reported (matches the full-tree Phase 3,
                                    // which only emits `edit` for files).
                                    let old_fs_id = if old_entry.mode & 0o40000 != 0 {
                                        Some(old_entry.id.clone())
                                    } else {
                                        None
                                    };
                                    next.push(Frame {
                                        old_fs_id,
                                        new_fs_id: new_entry.id.clone(),
                                        prefix: path,
                                    });
                                } else {
                                    // New side is a file with a different id:
                                    // always an edit, regardless of the old type.
                                    changes.push(FsChange {
                                        op_type: "edit",
                                        obj_type: "file",
                                        path: path.clone(),
                                        size: new_entry.size,
                                        obj_id: new_entry.id.clone(),
                                        old_path: None,
                                    });
                                    // If the old side was a directory, its
                                    // subtree is gone and must be deleted.
                                    if old_entry.mode & 0o40000 != 0 {
                                        next.push(Frame {
                                            old_fs_id: Some(old_entry.id.clone()),
                                            new_fs_id: EMPTY_SHA1.to_string(),
                                            prefix: path,
                                        });
                                    }
                                }
                            }
                        }
                    }

                    // Old-only entries: deleted outright.
                    for old_entry in &old_dir.dirents {
                        if new_dir.dirents.iter().any(|e| e.name == old_entry.name) {
                            continue;
                        }
                        let path = join_path(&frame.prefix, &old_entry.name);
                        changes.push(delete_change(&path, old_entry));
                        obj_to_deleted
                            .entry(old_entry.id.clone())
                            .or_default()
                            .push((path.clone(), old_entry.clone()));
                        if old_entry.mode & 0o40000 != 0 {
                            next.push(Frame {
                                old_fs_id: Some(old_entry.id.clone()),
                                new_fs_id: EMPTY_SHA1.to_string(),
                                prefix: path,
                            });
                        }
                    }
                }
                (None, None) => {}
            }
        }

        frontier = next;
    }

    // Match creates against deleted entries by obj_id to detect renames/moves.
    // Deletes are never un-emitted; a rename adds a rename/move change in
    // addition to the delete of the old path. When an obj_id appears on several
    // deleted paths, the last-collected path wins (deterministic frame order).
    for (path, entry) in created {
        let is_dir = entry.mode & 0o40000 != 0;
        if let Some(deleted_list) = obj_to_deleted.get_mut(&entry.id)
            && let Some((old_path, _old_entry)) = deleted_list.pop()
        {
            let old_name = file_name(&old_path);
            let new_name = file_name(&path);
            let op_type = if old_name == new_name {
                "move"
            } else {
                "rename"
            };
            changes.push(FsChange {
                op_type,
                obj_type: if is_dir { "dir" } else { "file" },
                path,
                size: entry.size,
                obj_id: entry.id,
                old_path: Some(old_path),
            });
        } else {
            changes.push(FsChange {
                op_type: "create",
                obj_type: if is_dir { "dir" } else { "file" },
                path,
                size: entry.size,
                obj_id: entry.id,
                old_path: None,
            });
        }
    }

    // Sort by path so output order is deterministic.
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// Join a directory prefix and entry name into an absolute path.
fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        format!("/{name}")
    } else {
        format!("{prefix}/{name}")
    }
}

/// Extract the final path segment of an absolute path.
fn file_name(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
}

/// Build a `delete` change for an old-side entry that no longer exists.
fn delete_change(path: &str, entry: &DirEntryData) -> FsChange {
    FsChange {
        op_type: "delete",
        obj_type: if entry.mode & 0o40000 != 0 {
            "dir"
        } else {
            "file"
        },
        path: path.to_string(),
        size: entry.size,
        obj_id: entry.id.clone(),
        old_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};
    use std::sync::Arc;

    const REPO: &str = "test-repo";

    async fn setup_diff_db() -> sea_orm::DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            "CREATE TABLE fs_objects (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id VARCHAR(36) NOT NULL,
                fs_id VARCHAR(40) NOT NULL,
                obj_type TINYINT NOT NULL,
                data TEXT NOT NULL
            );",
        ))
        .await
        .unwrap();
        db
    }

    async fn insert_dir(
        db: &sea_orm::DatabaseConnection,
        fs_id: &str,
        entries: &[(&str, bool, &str)],
    ) {
        let data = dir_data(entries);
        db.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!(
                "INSERT INTO fs_objects (repo_id, fs_id, obj_type, data) \
                 VALUES ('{REPO}', '{fs_id}', 3, '{data}')"
            ),
        ))
        .await
        .unwrap();
    }

    /// Build a directory-object data JSON from `(fs_id, is_dir, name)` triples.
    fn dir_data(entries: &[(&str, bool, &str)]) -> String {
        let items: Vec<String> = entries
            .iter()
            .map(|(id, is_dir, name)| {
                let mode = if *is_dir { 0o40000 } else { 0o100644 };
                format!(
                    r#"{{"id":"{id}","mode":{mode},"modifier":"u1","mtime":1000,"name":"{name}","size":0}}"#
                )
            })
            .collect();
        format!(
            r#"{{"dirents":[{}],"type":3,"version":1}}"#,
            items.join(",")
        )
    }

    /// Reference full-tree diff (the pre-incremental implementation) for
    /// equivalence checks against the incremental path.
    async fn diff_trees_full(
        repos: &Repositories,
        repo_id: &str,
        old_root_id: Option<&str>,
        new_root_id: &str,
    ) -> Result<Vec<FsChange>, AppError> {
        let old_root = old_root_id.unwrap();

        let mut old_entries: HashMap<String, DirEntryData> = HashMap::new();
        let mut new_entries: HashMap<String, DirEntryData> = HashMap::new();
        collect_entries(repos, repo_id, old_root, "", &mut old_entries).await?;
        collect_entries(repos, repo_id, new_root_id, "", &mut new_entries).await?;

        let mut changes = Vec::new();
        let mut obj_to_deleted: HashMap<&str, Vec<(&str, &DirEntryData)>> = HashMap::new();
        for (path, entry) in &old_entries {
            if !new_entries.contains_key(path) {
                let is_dir = entry.mode & 0o40000 != 0;
                changes.push(FsChange {
                    op_type: "delete",
                    obj_type: if is_dir { "dir" } else { "file" },
                    path: path.clone(),
                    size: entry.size,
                    obj_id: entry.id.clone(),
                    old_path: None,
                });
                obj_to_deleted
                    .entry(&entry.id)
                    .or_default()
                    .push((path.as_str(), entry));
            }
        }
        for (path, entry) in &new_entries {
            let is_dir = entry.mode & 0o40000 != 0;
            if let Some(deleted_list) = obj_to_deleted.get_mut(&entry.id.as_str())
                && let Some((old_path, _old_entry)) = deleted_list.pop()
            {
                let old_name = file_name(old_path);
                let new_name = file_name(path);
                let op_type = if old_name == new_name {
                    "move"
                } else {
                    "rename"
                };
                changes.push(FsChange {
                    op_type,
                    obj_type: if is_dir { "dir" } else { "file" },
                    path: path.clone(),
                    size: entry.size,
                    obj_id: entry.id.clone(),
                    old_path: Some(old_path.to_string()),
                });
                continue;
            }
            if !old_entries.contains_key(path) {
                changes.push(FsChange {
                    op_type: "create",
                    obj_type: if is_dir { "dir" } else { "file" },
                    path: path.clone(),
                    size: entry.size,
                    obj_id: entry.id.clone(),
                    old_path: None,
                });
            }
        }
        for (path, new_entry) in &new_entries {
            if let Some(old_entry) = old_entries.get(path) {
                let is_dir = new_entry.mode & 0o40000 != 0;
                if !is_dir && new_entry.id != old_entry.id {
                    changes.push(FsChange {
                        op_type: "edit",
                        obj_type: "file",
                        path: path.clone(),
                        size: new_entry.size,
                        obj_id: new_entry.id.clone(),
                        old_path: None,
                    });
                }
            }
        }
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(changes)
    }

    async fn assert_incremental_matches_full(repos: &Repositories, old_root: &str, new_root: &str) {
        let full = diff_trees_full(repos, REPO, Some(old_root), new_root)
            .await
            .unwrap();
        let inc = diff_trees(repos, REPO, Some(old_root), new_root)
            .await
            .unwrap();
        assert_eq!(inc, full, "incremental != full\nfull={full:?}\ninc={inc:?}");
    }

    #[tokio::test]
    async fn test_diff_edit() {
        let db = setup_diff_db().await;
        let repos = Repositories::new(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[("f2", false, "a.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_create() {
        let db = setup_diff_db().await;
        let repos = Repositories::new(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[]).await;
        insert_dir(&db, "root-new", &[("f2", false, "b.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_delete() {
        let db = setup_diff_db().await;
        let repos = Repositories::new(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_rename() {
        let db = setup_diff_db().await;
        let repos = Repositories::new(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[("f1", false, "b.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_move() {
        let db = setup_diff_db().await;
        let repos = Repositories::new(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("d1", true, "d")]).await;
        insert_dir(&db, "d1", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[("e1", true, "e")]).await;
        insert_dir(&db, "e1", &[("f1", false, "a.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_dir_rename_internal_unchanged() {
        let db = setup_diff_db().await;
        let repos = Repositories::new(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("d1", true, "d")]).await;
        insert_dir(&db, "d1", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[("d1", true, "e")]).await;
        insert_dir(&db, "d1", &[("f1", false, "a.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_dir_rename_internal_modified() {
        let db = setup_diff_db().await;
        let repos = Repositories::new(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("d1", true, "d")]).await;
        insert_dir(&db, "d1", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[("d2", true, "e")]).await;
        insert_dir(&db, "d2", &[("f1", false, "a.txt"), ("f3", false, "c.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_file_to_dir() {
        let db = setup_diff_db().await;
        let repos = Repositories::new(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("f1", false, "x")]).await;
        insert_dir(&db, "root-new", &[("d1", true, "x")]).await;
        insert_dir(&db, "d1", &[("f3", false, "c.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_dir_to_file() {
        let db = setup_diff_db().await;
        let repos = Repositories::new(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("d1", true, "x")]).await;
        insert_dir(&db, "d1", &[("f3", false, "c.txt")]).await;
        insert_dir(&db, "root-new", &[("f1", false, "x")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }
}
