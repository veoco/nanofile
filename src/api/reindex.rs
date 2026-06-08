use axum::{Json, extract::State};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{commit, repo};
use crate::error::AppError;

/// The well-known sentinel for empty directories in seafile's protocol.
const EMPTY_SHA1: &str = "0000000000000000000000000000000000000000";

#[derive(Deserialize)]
pub struct ReindexRequest {
    pub repo_id: String,
}

#[derive(Deserialize)]
pub struct IndexFileTextRequest {
    pub repo_id: String,
    pub path: String,
    pub text: String,
}

#[derive(Serialize)]
pub struct ReindexResponse {
    pub status: String,
    pub indexed: u64,
    pub skipped: u64,
}

#[derive(Serialize)]
pub struct IndexFileTextResponse {
    pub status: String,
}

/// Check that the authenticated user can access the given repo.
async fn check_repo_access(
    db: &DatabaseConnection,
    repo_id: &str,
    user_id: i32,
) -> Result<repo::Model, AppError> {
    let repo_model = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    if repo_model.owner_id != user_id {
        let is_member = crate::entity::repo_member::Entity::find()
            .filter(
                crate::entity::repo_member::Column::RepoId
                    .eq(repo_id)
                    .and(crate::entity::repo_member::Column::UserId.eq(user_id)),
            )
            .one(db)
            .await?
            .is_some();
        if !is_member {
            return Err(AppError::Forbidden);
        }
    }
    Ok(repo_model)
}

/// POST /api2/index-file-text/
///
/// Update the full-text search index for a specific file with custom text.
/// This is designed for files that cannot be automatically indexed (e.g.
/// images, PDFs). Use a vision model or other tool to extract text, then
/// upload it via this endpoint to associate it with the file.
///
/// If the file already has an index entry (from a previous upload or call),
/// it is replaced.
///
/// Request:
/// ```json
/// {
///   "repo_id": "<uuid>",
///   "path": "/photos/screenshot.png",
///   "text": "text extracted from the image by a vision model"
/// }
/// ```
pub async fn index_file_text(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<IndexFileTextRequest>,
) -> Result<Json<IndexFileTextResponse>, AppError> {
    if req.path.is_empty() {
        return Err(AppError::BadRequest("path is required".into()));
    }
    if req.text.is_empty() {
        return Err(AppError::BadRequest("text is required".into()));
    }

    let db = state.db.as_ref();

    // Verify access to the repo.
    check_repo_access(db, &req.repo_id, auth.user_id).await?;

    let indexer = state
        .indexer
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("full-text indexing is not enabled".into()))?;

    // Normalize path to start with /.
    let fullpath = if req.path.starts_with('/') {
        req.path.clone()
    } else {
        format!("/{}", req.path)
    };

    // Extract filename from path.
    let filename = fullpath
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(&fullpath);

    // Index (deletes any existing entry for this path, then adds new one).
    if let Err(e) = indexer.index_file(&req.repo_id, &fullpath, filename, &req.text) {
        tracing::warn!("Failed to index file text for {fullpath}: {e}");
        return Err(AppError::Internal(format!("index failed: {e}")));
    }

    Ok(Json(IndexFileTextResponse {
        status: "ok".to_string(),
    }))
}

/// POST /api2/reindex/
///
/// Rebuild the full-text search index for all files in a repository.
/// Traverses the entire FS object tree and re-indexes every file.
pub async fn reindex(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReindexRequest>,
) -> Result<Json<ReindexResponse>, AppError> {
    let db = state.db.as_ref();
    let repo_id = &req.repo_id;

    // Verify the user has access to this repo.
    let repo_model = check_repo_access(db, repo_id, auth.user_id).await?;

    let indexer = state
        .indexer
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("full-text indexing is not enabled".into()))?;

    // Get the head commit root ID.
    let head_commit_id = repo_model
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("repo has no commits".into()))?;
    let head = commit::Entity::find()
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    if head.root_id == EMPTY_SHA1 {
        return Ok(Json(ReindexResponse {
            status: "ok".to_string(),
            indexed: 0,
            skipped: 0,
        }));
    }

    // Collect all file paths by traversing the FS tree.
    let mut file_paths: Vec<String> = Vec::new();
    collect_file_paths(db, repo_id, &head.root_id, "", &mut file_paths).await?;

    // Re-index each file.
    let mut indexed = 0u64;
    let mut skipped = 0u64;

    for fullpath in &file_paths {
        match indexer
            .reindex_file(db, repo_id, fullpath, &state.block_store)
            .await
        {
            Ok(()) => indexed += 1,
            Err(_) => skipped += 1,
        }
    }

    Ok(Json(ReindexResponse {
        status: "ok".to_string(),
        indexed,
        skipped,
    }))
}

/// Recursively collect all file paths from the FS tree.
async fn collect_file_paths(
    db: &DatabaseConnection,
    repo_id: &str,
    root_fs_id: &str,
    base_path: &str,
    results: &mut Vec<String>,
) -> Result<(), AppError> {
    let mut stack: Vec<(String, String)> = vec![(root_fs_id.to_string(), base_path.to_string())];

    while let Some((fs_id, path)) = stack.pop() {
        if fs_id == EMPTY_SHA1 {
            continue;
        }

        let dir_data = match crate::storage::read_fs_dir_data(db, repo_id, &fs_id).await {
            Ok(data) => data,
            Err(_) => continue,
        };

        for entry in &dir_data.dirents {
            let full_path = if path.is_empty() {
                format!("/{}", entry.name)
            } else if path.starts_with('/') {
                format!("{}/{}", path, entry.name)
            } else {
                format!("/{}/{}", path, entry.name)
            };

            if entry.mode & crate::serialization::S_IFDIR != 0 {
                // Recurse into subdirectories
                stack.push((entry.id.clone(), full_path));
            } else {
                // File — record for re-indexing
                results.push(full_path);
            }
        }
    }

    Ok(())
}
