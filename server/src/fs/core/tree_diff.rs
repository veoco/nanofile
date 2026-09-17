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
    /// True on a `"delete"` whose old path was reported as a `"rename"` or
    /// `"move"` in the same result set (i.e. the same obj_id was re-emitted at
    /// a new path). The delete is kept so consumers can still clean up the old
    /// path, but activity logging must skip it: official seafevents records a
    /// rename as a single event.
    pub superseded: bool,
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

    let mut guard = crate::fs::core::traversal::TreeGuard::new();
    while !frontier.is_empty() {
        guard.enter_level()?;
        guard.visit(frontier.len())?;
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
                    superseded: false,
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
    // obj_id. A delete whose obj_id is re-emitted at a new path is then marked
    // `superseded` so consumers can tell it apart from a real delete.
    struct Frame {
        old_fs_id: Option<String>,
        new_fs_id: String,
        prefix: String,
    }

    let mut changes: Vec<FsChange> = Vec::new();
    // New entries pending rename/move matching, then create fallback.
    let mut created: Vec<(String, DirEntryData)> = Vec::new();
    // Old paths removed from the tree. Deletes are collected here instead of
    // being pushed into `changes` straight away: a delete found by a later
    // descent can still turn out to be the old side of a rename/move, and the
    // matching pass below must be able to see every delete before it consumes
    // one. They are merged into `changes` afterwards.
    let mut pending_deletes: Vec<FsChange> = Vec::new();
    // obj_id → old-side entries removed from their path, for rename/move matching.
    let mut obj_to_deleted: HashMap<String, Vec<(String, DirEntryData)>> = HashMap::new();

    let mut frontier: Vec<Frame> = vec![Frame {
        old_fs_id: Some(old_root.to_string()),
        new_fs_id: new_root_id.to_string(),
        prefix: String::new(),
    }];

    let mut guard = crate::fs::core::traversal::TreeGuard::new();
    while !frontier.is_empty() {
        guard.enter_level()?;
        guard.visit(frontier.len())?;
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
                        pending_deletes.push(delete_change(&path, entry));
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

                    // Old-only entries are removed from this directory first.
                    // They must be recorded before the new entries are examined:
                    // whether a new directory is a reused (unchanged) object
                    // depends on what this frame is dropping, and a renamed
                    // directory's entry is one of them.
                    let mut deleted_here: Vec<(String, DirEntryData)> = Vec::new();
                    for old_entry in &old_dir.dirents {
                        if new_dir.dirents.iter().any(|e| e.name == old_entry.name) {
                            continue;
                        }
                        let path = join_path(&frame.prefix, &old_entry.name);
                        pending_deletes.push(delete_change(&path, old_entry));
                        deleted_here.push((path.clone(), old_entry.clone()));
                        if old_entry.mode & 0o40000 != 0 {
                            next.push(Frame {
                                old_fs_id: Some(old_entry.id.clone()),
                                new_fs_id: EMPTY_SHA1.to_string(),
                                prefix: path,
                            });
                        }
                    }
                    // Index this frame's removed entries by object id, so a new
                    // entry carrying one of those ids is recognised as a
                    // rename/move.
                    let mut deleted_by_id: HashMap<&str, &DirEntryData> = HashMap::new();
                    for (_, entry) in &deleted_here {
                        deleted_by_id.entry(entry.id.as_str()).or_insert(entry);
                    }
                    for (path, entry) in &deleted_here {
                        obj_to_deleted
                            .entry(entry.id.clone())
                            .or_default()
                            .push((path.clone(), entry.clone()));
                    }

                    for new_entry in &new_dir.dirents {
                        let path = join_path(&frame.prefix, &new_entry.name);
                        let new_is_dir = new_entry.mode & 0o40000 != 0;
                        let old_entry = old_entries.get(new_entry.name.as_str()).copied();
                        let deleted_entry = deleted_by_id.get(new_entry.id.as_str()).copied();
                        match old_entry {
                            // Same path, same object: nothing changed.
                            Some(old_entry) if new_entry.id == old_entry.id => {}
                            // Same path, both directories, different id: descend
                            // both sides; the directory itself is not reported.
                            Some(old_entry) if new_is_dir && old_entry.mode & 0o40000 != 0 => {
                                next.push(Frame {
                                    old_fs_id: Some(old_entry.id.clone()),
                                    new_fs_id: new_entry.id.clone(),
                                    prefix: path,
                                });
                            }
                            // A different old path dropped this exact directory
                            // object: a rename/move of the whole subtree. The
                            // pairing pass reports the directory itself, so do
                            // not descend — its contents are byte-identical.
                            None if new_is_dir && deleted_entry.is_some() => {
                                created.push((path.clone(), new_entry.clone()));
                            }
                            // Same path or new path, but not a reused directory:
                            // a create, or an edit when a file replaced a file.
                            _ => {
                                let old_was_dir = old_entry.is_some_and(|d| d.mode & 0o40000 != 0);
                                if old_entry.is_none() {
                                    // Genuinely new path.
                                    created.push((path.clone(), new_entry.clone()));
                                }
                                if new_is_dir {
                                    next.push(Frame {
                                        old_fs_id: old_entry.map(|d| d.id.clone()),
                                        new_fs_id: new_entry.id.clone(),
                                        prefix: path,
                                    });
                                } else if let Some(old_entry) = old_entry {
                                    // New side is a file with a different id:
                                    // always an edit, regardless of the old type.
                                    changes.push(FsChange {
                                        op_type: "edit",
                                        obj_type: "file",
                                        path: path.clone(),
                                        size: new_entry.size,
                                        obj_id: new_entry.id.clone(),
                                        old_path: None,
                                        superseded: false,
                                    });
                                    // If the old side was a directory, its
                                    // subtree is gone and must be deleted.
                                    if old_was_dir {
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
                }
                (None, None) => {}
            }
        }

        frontier = next;
    }

    // Deletes are emitted only now, after the walk has seen the whole changed
    // tree, so the rename/move matching below can pair against every one of
    // them regardless of the order the frontier happened to visit them in.
    changes.extend(pending_deletes);

    // Match creates against deleted entries by obj_id to detect renames/moves,
    // mirroring seafevents' `CommitDiffer(handle_rename=True)`: an entry that
    // moved to a new path is reported once, as a rename (same parent directory)
    // or a move (different parent directory). When an obj_id appears on several
    // deleted paths, the last-collected path wins (deterministic frame order).
    let mut superseded_ids: Vec<String> = Vec::new();
    // Directory objects reused under a new path. Every entry below them moved
    // with the directory, so the deletes recorded under their old path are
    // superseded too.
    let mut reused_dir_old_paths: Vec<String> = Vec::new();
    for (path, entry) in created {
        let is_dir = entry.mode & 0o40000 != 0;
        if let Some(deleted_list) = obj_to_deleted.get_mut(&entry.id)
            && let Some((old_path, _old_entry)) = deleted_list.pop()
        {
            let op_type = if parent_dir(&old_path) == parent_dir(&path) {
                "rename"
            } else {
                "move"
            };
            superseded_ids.push(entry.id.clone());
            if is_dir {
                reused_dir_old_paths.push(old_path.clone());
            }
            changes.push(FsChange {
                op_type,
                obj_type: if is_dir { "dir" } else { "file" },
                path,
                size: entry.size,
                obj_id: entry.id,
                old_path: Some(old_path),
                superseded: false,
            });
        } else {
            changes.push(FsChange {
                op_type: "create",
                obj_type: if is_dir { "dir" } else { "file" },
                path,
                size: entry.size,
                obj_id: entry.id,
                old_path: None,
                superseded: false,
            });
        }
    }

    // Mark the delete of each matched old path. The delete stays in the result
    // (the indexer needs it to drop the old path) but is flagged so that
    // activity logging can skip it: a rename is one event, not rename + delete.
    if !superseded_ids.is_empty() {
        for c in changes.iter_mut() {
            if c.op_type != "delete" {
                continue;
            }
            // A delete is superseded when its own object was re-emitted at a new
            // path, or when it sits under a directory that was: the whole
            // subtree moved with that directory.
            let under_reused_dir = reused_dir_old_paths.iter().any(|dir| {
                c.path.len() > dir.len()
                    && c.path.starts_with(dir.as_str())
                    && c.path.as_bytes()[dir.len()] == b'/'
            });
            if under_reused_dir || superseded_ids.iter().any(|id| id == &c.obj_id) {
                c.superseded = true;
            }
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

/// Extract the parent directory of an absolute path (`"/"` for a top-level
/// entry).
fn parent_dir(path: &str) -> &str {
    match path.rsplit_once('/') {
        None => "/",
        Some((_, "")) => path,
        Some(("", _)) => "/",
        Some((dir, _)) => dir,
    }
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
        superseded: false,
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
        db.execute_raw(Statement::from_string(
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
        db.execute_raw(Statement::from_string(
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
        // Old paths not present in the new tree, plus old directory objects
        // that survived at the same path with a changed id. Both are candidate
        // old sides of a rename/move; the matching pass below consumes the ones
        // that were actually reused.
        let mut obj_to_deleted: HashMap<&str, Vec<(&str, &DirEntryData)>> = HashMap::new();
        for (path, entry) in &old_entries {
            let is_dir = entry.mode & 0o40000 != 0;
            let changed_dir_at_same_path = is_dir
                && new_entries
                    .get(path)
                    .is_some_and(|new_entry| new_entry.id != entry.id);
            if !new_entries.contains_key(path) || changed_dir_at_same_path {
                if !new_entries.contains_key(path) {
                    changes.push(FsChange {
                        op_type: "delete",
                        obj_type: if is_dir { "dir" } else { "file" },
                        path: path.clone(),
                        size: entry.size,
                        obj_id: entry.id.clone(),
                        old_path: None,
                        superseded: false,
                    });
                }
                obj_to_deleted
                    .entry(&entry.id)
                    .or_default()
                    .push((path.as_str(), entry));
            }
        }

        // Paths whose subtree must not be re-reported: a directory object reused
        // at another path is byte-identical, so everything beneath it is
        // unchanged and the rename/move of the directory alone describes the
        // whole move.
        let mut reused_dirs: Vec<(String, &DirEntryData, &str)> = Vec::new();
        let mut superseded_ids: Vec<&str> = Vec::new();
        for (path, entry) in &new_entries {
            let is_dir = entry.mode & 0o40000 != 0;
            if let Some(deleted_list) = obj_to_deleted.get_mut(&entry.id.as_str())
                && let Some((old_path, _old_entry)) = deleted_list.pop()
            {
                let op_type = if parent_dir(old_path) == parent_dir(path) {
                    "rename"
                } else {
                    "move"
                };
                superseded_ids.push(entry.id.as_str());
                if is_dir {
                    reused_dirs.push((path.clone(), entry, old_path));
                }
                changes.push(FsChange {
                    op_type,
                    obj_type: if is_dir { "dir" } else { "file" },
                    path: path.clone(),
                    size: entry.size,
                    obj_id: entry.id.clone(),
                    old_path: Some(old_path.to_string()),
                    superseded: false,
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
                    superseded: false,
                });
            }
        }

        // Everything beneath a reused directory object is unchanged, so it must
        // not be re-reported. Mirrors the incremental walk's decision not to
        // descend into such a directory.
        let in_reused_subtree = |path: &str| {
            reused_dirs.iter().any(|(new_path, _, _)| {
                path.len() > new_path.len()
                    && path.starts_with(new_path.as_str())
                    && path.as_bytes()[new_path.len()] == b'/'
            })
        };
        changes.retain(|c| !in_reused_subtree(&c.path));

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
                        superseded: false,
                    });
                }
            }
        }
        changes.retain(|c| !in_reused_subtree(&c.path));
        for c in changes.iter_mut() {
            if c.op_type == "delete" && superseded_ids.iter().any(|id| *id == c.obj_id) {
                c.superseded = true;
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

    /// Assert exactly which changes a diff produces, as
    /// `"op_type obj_type path[ old_path]"`, plus whether the entry is a
    /// superseded delete.
    async fn assert_changes(
        repos: &Repositories,
        old_root: &str,
        new_root: &str,
        expected: &[(&str, bool)],
    ) {
        let changes = diff_trees(repos, REPO, Some(old_root), new_root)
            .await
            .unwrap();
        let actual: Vec<(String, bool)> = changes
            .iter()
            .map(|c| {
                let mut s = format!("{} {} {}", c.op_type, c.obj_type, c.path);
                if let Some(op) = c.old_path.as_deref() {
                    s.push(' ');
                    s.push_str(op);
                }
                (s, c.superseded)
            })
            .collect();
        let expected: Vec<(String, bool)> = expected
            .iter()
            .map(|(s, sup)| ((*s).to_string(), *sup))
            .collect();
        assert_eq!(
            actual, expected,
            "unexpected diff for {old_root} -> {new_root}"
        );
    }

    #[tokio::test]
    async fn test_diff_edit() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[("f2", false, "a.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_create() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[]).await;
        insert_dir(&db, "root-new", &[("f2", false, "b.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_delete() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_rename() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[("f1", false, "b.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
        // A rename keeps the file in the same parent directory. The delete of
        // the old path is superseded — activity logging must skip it.
        assert_changes(
            &repos,
            "root-old",
            "root-new",
            &[
                ("delete file /a.txt", true),
                ("rename file /b.txt /a.txt", false),
            ],
        )
        .await;
    }

    #[tokio::test]
    async fn test_diff_move() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("d1", true, "d")]).await;
        insert_dir(&db, "d1", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[("e1", true, "e")]).await;
        insert_dir(&db, "e1", &[("f1", false, "a.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    /// A file moved to a different directory is a `move`, not a `rename`
    /// (matching seafevents' `commit_differ.py`, which compares the parent
    /// directory).
    #[tokio::test]
    async fn test_diff_file_move_to_other_dir_is_move() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[("d1", true, "d")]).await;
        insert_dir(&db, "d1", &[("f1", false, "a.txt")]).await;
        assert_changes(
            &repos,
            "root-old",
            "root-new",
            &[
                ("delete file /a.txt", true),
                ("create dir /d", false),
                ("move file /d/a.txt /a.txt", false),
            ],
        )
        .await;
    }

    /// A pure delete (no path reuses the object id) stays a plain delete.
    #[tokio::test]
    async fn test_diff_delete_is_not_superseded() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[]).await;
        assert_changes(
            &repos,
            "root-old",
            "root-new",
            &[("delete file /a.txt", false)],
        )
        .await;
    }

    #[tokio::test]
    async fn test_diff_dir_rename_internal_unchanged() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("d1", true, "d")]).await;
        insert_dir(&db, "d1", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[("d1", true, "e")]).await;
        insert_dir(&db, "d1", &[("f1", false, "a.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
        // The child directory object is reused unchanged, so the rename alone
        // describes the whole move: no per-child move events. The deletes of
        // the old path and of the entries under it are all superseded.
        assert_changes(
            &repos,
            "root-old",
            "root-new",
            &[
                ("delete dir /d", true),
                ("delete file /d/a.txt", true),
                ("rename dir /e /d", false),
            ],
        )
        .await;
    }

    #[tokio::test]
    async fn test_diff_dir_rename_internal_modified() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("d1", true, "d")]).await;
        insert_dir(&db, "d1", &[("f1", false, "a.txt")]).await;
        insert_dir(&db, "root-new", &[("d2", true, "e")]).await;
        insert_dir(&db, "d2", &[("f1", false, "a.txt"), ("f3", false, "c.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
        // The dir object id changed (a child was added), so the directory
        // itself is no longer a rename: it is reported as a delete + create.
        // The unchanged child keeps its id and is still matched as a move.
        assert_changes(
            &repos,
            "root-old",
            "root-new",
            &[
                ("delete dir /d", false),
                ("delete file /d/a.txt", true),
                ("create dir /e", false),
                ("move file /e/a.txt /d/a.txt", false),
                ("create file /e/c.txt", false),
            ],
        )
        .await;
    }

    #[tokio::test]
    async fn test_diff_file_to_dir() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("f1", false, "x")]).await;
        insert_dir(&db, "root-new", &[("d1", true, "x")]).await;
        insert_dir(&db, "d1", &[("f3", false, "c.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    #[tokio::test]
    async fn test_diff_dir_to_file() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("d1", true, "x")]).await;
        insert_dir(&db, "d1", &[("f3", false, "c.txt")]).await;
        insert_dir(&db, "root-new", &[("f1", false, "x")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
    }

    /// When a directory is renamed and one of its subdirectories keeps its
    /// object id, the subdirectory's contents did not change either, so they
    /// must not be reported as moves.
    #[tokio::test]
    async fn test_diff_nested_unchanged_dir_not_reported() {
        let db = setup_diff_db().await;
        let repos = Repositories::new_for_tests(Arc::new(db.clone()));
        insert_dir(&db, "root-old", &[("d1", true, "a")]).await;
        insert_dir(&db, "d1", &[("d2", true, "b")]).await;
        insert_dir(&db, "d2", &[("f1", false, "f.txt")]).await;
        // /a → /c, with b's object reused verbatim.
        insert_dir(&db, "root-new", &[("d1", true, "c")]).await;
        insert_dir(&db, "d1", &[("d2", true, "b")]).await;
        insert_dir(&db, "d2", &[("f1", false, "f.txt")]).await;
        assert_incremental_matches_full(&repos, "root-old", "root-new").await;
        assert_changes(
            &repos,
            "root-old",
            "root-new",
            &[
                ("delete dir /a", true),
                ("delete dir /a/b", true),
                ("delete file /a/b/f.txt", true),
                ("rename dir /c /a", false),
            ],
        )
        .await;
    }
}
