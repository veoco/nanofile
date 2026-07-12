use std::collections::HashMap;

use crate::common::EMPTY_SHA1;
use crate::error::AppError;
use crate::repo::fs_tree::read_fs_dir_data;
use crate::repository::Repositories;
use base::common::DirEntryData;

/// A single file-system change detected by diffing two tree snapshots.
#[derive(Debug, Clone)]
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

/// Walk an FS tree from `root_fs_id` using an explicit stack (no recursion)
/// and populate `out` with every entry's path → (DirEntryData).
/// Directories are included too.
async fn collect_entries(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
    prefix: &str,
    out: &mut HashMap<String, DirEntryData>,
) -> Result<(), AppError> {
    struct StackFrame {
        fs_id: String,
        prefix: String,
    }

    let mut stack = vec![StackFrame {
        fs_id: root_fs_id.to_string(),
        prefix: prefix.to_string(),
    }];

    while let Some(frame) = stack.pop() {
        let dir = read_fs_dir_data(repos, repo_id, &frame.fs_id).await?;
        for entry in &dir.dirents {
            let entry_path = if frame.prefix.is_empty() {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", frame.prefix, entry.name)
            };
            out.insert(entry_path.clone(), entry.clone());

            // Push subdirectories onto the stack for further traversal.
            if entry.mode & 0o40000 != 0 {
                stack.push(StackFrame {
                    fs_id: entry.id.clone(),
                    prefix: entry_path,
                });
            }
        }
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
    let no_old_tree = old_root_id.is_none()
        || old_root_id == Some(EMPTY_SHA1)
        || old_root_id == Some("0000000000000000000000000000000000000000");

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

    let mut old_entries: HashMap<String, DirEntryData> = HashMap::new();
    let mut new_entries: HashMap<String, DirEntryData> = HashMap::new();
    collect_entries(repos, repo_id, old_root, "", &mut old_entries).await?;
    collect_entries(repos, repo_id, new_root_id, "", &mut new_entries).await?;

    let mut changes = Vec::new();

    // Phase 1: detect deletes + compute obj_id → [(old_path, entry)] for rename/move matching.
    let mut obj_to_deleted: HashMap<&str, Vec<(&str, &DirEntryData)>> = HashMap::new();
    for (path, entry) in &old_entries {
        if !new_entries.contains_key(path) {
            // Entry removed entirely (not just moved/renamed).
            let is_dir = entry.mode & 0o40000 != 0;
            changes.push(FsChange {
                op_type: "delete",
                obj_type: if is_dir { "dir" } else { "file" },
                path: path.clone(),
                size: entry.size,
                obj_id: entry.id.clone(),
                old_path: None,
            });
            // Also index by obj_id for rename/move matching.
            obj_to_deleted
                .entry(&entry.id)
                .or_default()
                .push((path.as_str(), entry));
        }
    }

    // Phase 2: detect creates + renames + moves.
    for (path, entry) in &new_entries {
        let is_dir = entry.mode & 0o40000 != 0;

        if let Some(deleted_list) = obj_to_deleted.get_mut(&entry.id.as_str())
            && let Some((old_path, _old_entry)) = deleted_list.pop()
        {
            // Same obj_id → content preserved, so this is a rename or move.
            let old_name = std::path::Path::new(old_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let new_name = std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            if old_name == new_name {
                // Same name, different parent → move.
                changes.push(FsChange {
                    op_type: "move",
                    obj_type: if is_dir { "dir" } else { "file" },
                    path: path.clone(),
                    size: entry.size,
                    obj_id: entry.id.clone(),
                    old_path: Some(old_path.to_string()),
                });
            } else {
                // Different name → rename.
                changes.push(FsChange {
                    op_type: "rename",
                    obj_type: if is_dir { "dir" } else { "file" },
                    path: path.clone(),
                    size: entry.size,
                    obj_id: entry.id.clone(),
                    old_path: Some(old_path.to_string()),
                });
            }
            continue;
        }

        // Not a rename/move → check if it's a genuine create.
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

    // Phase 3: detect edits (same path, different fs_id, not a directory).
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

    // Sort by path so output order is deterministic.
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}
