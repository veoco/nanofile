use std::collections::HashMap;

use sea_orm::DatabaseConnection;

use crate::common::EMPTY_SHA1;
use crate::repo::fs_tree::read_fs_dir_data;
use crate::serialization::fs_json::DirEntryData;

/// A single file-system change detected by diffing two tree snapshots.
#[derive(Debug, Clone)]
pub struct FsChange {
    /// `"create"`, `"delete"`, or `"edit"`
    pub op_type: &'static str,
    /// `"file"` or `"dir"`
    pub obj_type: &'static str,
    /// Absolute path of the affected entry (e.g. `/docs/intro.md`).
    pub path: String,
    /// File size in bytes (0 for directories).
    pub size: i64,
    /// fs_object ID (SHA1).
    pub obj_id: String,
}

/// Walk an FS tree from `root_fs_id` using an explicit stack (no recursion)
/// and populate `out` with every entry's path → (DirEntryData).
/// Directories are included too.
async fn collect_entries(
    db: &DatabaseConnection,
    repo_id: &str,
    root_fs_id: &str,
    prefix: &str,
    out: &mut HashMap<String, DirEntryData>,
) -> Result<(), Box<dyn std::error::Error>> {
    struct StackFrame {
        fs_id: String,
        prefix: String,
    }

    let mut stack = vec![StackFrame {
        fs_id: root_fs_id.to_string(),
        prefix: prefix.to_string(),
    }];

    while let Some(frame) = stack.pop() {
        let dir = read_fs_dir_data(db, repo_id, &frame.fs_id).await?;
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
    db: &DatabaseConnection,
    repo_id: &str,
    old_root_id: Option<&str>,
    new_root_id: &str,
) -> Result<Vec<FsChange>, Box<dyn std::error::Error>> {
    // If there is no old tree or it is the empty sentinel, everything is new.
    let no_old_tree = old_root_id.is_none()
        || old_root_id == Some(EMPTY_SHA1)
        || old_root_id == Some("0000000000000000000000000000000000000000");

    if no_old_tree {
        let mut entries = HashMap::new();
        collect_entries(db, repo_id, new_root_id, "", &mut entries).await?;
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
    collect_entries(db, repo_id, old_root, "", &mut old_entries).await?;
    collect_entries(db, repo_id, new_root_id, "", &mut new_entries).await?;

    let mut changes = Vec::new();

    // Added entries.
    for (path, entry) in &new_entries {
        if !old_entries.contains_key(path) {
            let is_dir = entry.mode & 0o40000 != 0;
            changes.push(FsChange {
                op_type: "create",
                obj_type: if is_dir { "dir" } else { "file" },
                path: path.clone(),
                size: entry.size,
                obj_id: entry.id.clone(),
            });
        }
    }

    // Deleted entries.
    for (path, entry) in &old_entries {
        if !new_entries.contains_key(path) {
            let is_dir = entry.mode & 0o40000 != 0;
            changes.push(FsChange {
                op_type: "delete",
                obj_type: if is_dir { "dir" } else { "file" },
                path: path.clone(),
                size: entry.size,
                obj_id: entry.id.clone(),
            });
        }
    }

    // Modified files (same path, different fs_id, not a directory).
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
                });
            }
        }
    }

    // Sort by path so output order is deterministic.
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}
