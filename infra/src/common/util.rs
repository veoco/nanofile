use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::entity::{commit, repo};
use crate::serialization::fs_json::DirEntryData;
use base::AppError;

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
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;
    let head_commit_id = repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("No commits yet".to_string()))?;
    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".to_string()))?;
    Ok(head.root_id)
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
