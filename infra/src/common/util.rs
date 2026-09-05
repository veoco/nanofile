use std::collections::HashMap;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::entity::{commit, repo};
use base::AppError;
use base::common::DirEntryData;

/// Extract a field from a POST body probing JSON, form-urlencoded,
/// then multipart/form-data in order.
pub fn extract_body_field(bytes: &[u8], field_name: &str) -> Option<String> {
    // Try JSON
    if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(bytes)
        && let Some(val) = map.get(field_name)
    {
        return Some(val.clone());
    }
    // Try form-urlencoded
    if let Ok(map) = serde_urlencoded::from_bytes::<HashMap<String, String>>(bytes)
        && let Some(val) = map.get(field_name)
    {
        return Some(val.clone());
    }
    // Try multipart
    extract_multipart_field(bytes, field_name)
}

/// Extract a named field from a multipart/form-data body by scanning the
/// raw body for `name="<field_name>"` and returning the value that follows
/// the header-terminating `\r\n\r\n` boundary.
pub fn extract_multipart_field(bytes: &[u8], field_name: &str) -> Option<String> {
    let body_str = String::from_utf8_lossy(bytes);
    let pattern = format!("name=\"{}\"", field_name);
    let rest = body_str.split(&pattern).nth(1)?;
    // The value follows after the part headers which end with \r\n\r\n
    let val_block = rest.split("\r\n\r\n").nth(1)?;
    let value = val_block.split("\r\n").next().unwrap_or("").trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Get the root_fs_id from the repo's head commit for path resolution.
pub async fn get_head_root_id(db: &DatabaseConnection, repo_id: &str) -> Result<String, AppError> {
    get_head_root_id_opt(db, repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("No commits yet".to_string()))
}

/// Like `get_head_root_id` but returns `Ok(None)` when the repo has no head
/// commit yet (vs. an error for a missing repo or head commit record).
pub async fn get_head_root_id_opt(
    db: &DatabaseConnection,
    repo_id: &str,
) -> Result<Option<String>, AppError> {
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;
    let Some(head_commit_id) = repo_record.head_commit_id else {
        return Ok(None);
    };
    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".to_string()))?;
    Ok(Some(head.root_id))
}

/// Get the head commit ID for a repo, without resolving the root fs_id.
/// Returns an error if the repo or head commit doesn't exist.
pub async fn get_head_commit_id(
    db: &DatabaseConnection,
    repo_id: &str,
) -> Result<String, AppError> {
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;
    repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("No commits yet".to_string()))
}

/// Extract the parent directory path from a full path.
/// `/dir/file.txt` → `/dir`,  `/file.txt` → `/`
pub fn parent_path_from(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "/",
    }
}

/// Extract the final path segment (filename or directory name).
/// `/dir/file.txt` → `file.txt`,  `/dir/` → `""`,  `/` → `""`
pub fn basename(path: &str) -> &str {
    path.rsplit_once('/').map(|(_, name)| name).unwrap_or("")
}

/// Join a parent path and a name, avoiding a doubled slash at the root.
/// `("/", "a")` → `/a`,  `("/dir", "a")` → `/dir/a`
pub fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// Format a Unix timestamp (seconds) as an RFC3339 string, or `""` when the
/// timestamp cannot be represented as a `DateTime`.
pub fn timestamp_rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

/// Format a byte count as a human-readable size (`B`/`KB`/`MB`/`GB`/`TB`).
/// Uses decimal (1000-based) units, matching the frontend `formatFileSize`.
pub fn format_size(size: i64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut s = size as f64;
    let mut unit = 0;
    while s >= 1000.0 && unit < UNITS.len() - 1 {
        s /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", size, UNITS[unit])
    } else {
        format!("{:.1} {}", s, UNITS[unit])
    }
}

/// Generate a unique filename when there's a name collision.
/// Appends " (N)" before the extension, e.g. "file (1).txt", "file (2).txt".
pub fn generate_unique_filename(existing: &[DirEntryData], name: &str) -> String {
    let base = if let Some(dot) = name.rfind('.') {
        let (stem, ext) = name.split_at(dot);
        (stem.to_string(), ext.to_string())
    } else {
        (name.to_string(), String::new())
    };

    let mut i = 1;
    loop {
        let candidate = if base.1.is_empty() {
            format!("{} ({})", base.0, i)
        } else {
            format!("{} ({}){}", base.0, i, base.1)
        };
        if !existing.iter().any(|d| d.name == candidate) {
            return candidate;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn format_size_uses_decimal_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1000), "1.0 KB");
        assert_eq!(format_size(999_999), "1000.0 KB");
        assert_eq!(format_size(1_000_000), "1.0 MB");
        assert_eq!(format_size(1_000_000_000), "1.0 GB");
        assert_eq!(format_size(1_000_000_000_000), "1.0 TB");
    }
}
