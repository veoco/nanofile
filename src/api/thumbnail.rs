use axum::{
    Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{commit, fs_object, repo, thumbnail as thumbnail_entity};
use crate::error::AppError;
use crate::serialization::fs_json::SEAF_METADATA_TYPE_DIR;
use crate::storage::download::Downloader;

#[derive(Deserialize)]
pub struct ThumbnailQuery {
    pub p: Option<String>,
    pub size: Option<u32>,
}

/// `GET /api2/repos/{repo_id}/thumbnail/?p=/path&size=48`
///
/// Returns a thumbnail image for the given file. Generates one on-the-fly
/// if it doesn't already exist in the thumbnails cache.
pub async fn get_thumbnail(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<ThumbnailQuery>,
) -> Result<Response, AppError> {
    let path = query
        .p
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let size = query.size.unwrap_or(48);

    // Normalize path
    let normalized_path = if path.is_empty() || path == "/" {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };

    // Verify the path exists and is a file via the FS tree.
    let repo_model = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;
    let head_commit_id = repo_model
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("No commits yet".into()))?;
    let head_commit = commit::Entity::find()
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".into()))?;

    let file_fs_id = crate::storage::resolve_fs_id(
        state.db.as_ref(),
        &repo_id,
        &head_commit.root_id,
        &normalized_path,
        None,
    )
    .await
    .map_err(|_| AppError::NotFound("file not found".into()))?;

    let file_obj = if file_fs_id == "0000000000000000000000000000000000000000" {
        // EMPTY_SHA1 is the sentinel for empty directories — no fs_object record exists.
        return Err(AppError::BadRequest("path is a directory".into()));
    } else {
        fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(&repo_id))
            .filter(fs_object::Column::FsId.eq(&file_fs_id))
            .one(state.db.as_ref())
            .await?
            .ok_or_else(|| AppError::NotFound("file not found".into()))?
    };

    if file_obj.obj_type == SEAF_METADATA_TYPE_DIR as i8 {
        return Err(AppError::BadRequest("path is a directory".into()));
    }

    let file_name = normalized_path
        .rsplit_once('/')
        .map(|(_, n)| n)
        .unwrap_or("file")
        .to_string();

    // Check if thumbnail already exists in cache
    let existing = thumbnail_entity::Entity::find()
        .filter(thumbnail_entity::Column::RepoId.eq(&repo_id))
        .filter(thumbnail_entity::Column::Path.eq(&normalized_path))
        .filter(thumbnail_entity::Column::Size.eq(size as i32))
        .one(state.db.as_ref())
        .await?;

    if let Some(_t) = existing {
        // Return cached thumbnail as PNG
        let thumbnail_path = state
            .block_dir
            .parent()
            .unwrap_or(&state.block_dir)
            .join("thumbnails")
            .join(&repo_id)
            .join(format!(
                "{}_{}.png",
                normalize_path_for_file(&normalized_path),
                size
            ));
        if thumbnail_path.exists() {
            let data = tokio::fs::read(&thumbnail_path).await?;
            return Ok(
                (StatusCode::OK, [(header::CONTENT_TYPE, "image/png")], data).into_response(),
            );
        }
    }

    // Generate thumbnail from file content
    // First check if this is an image file by extension
    let ext = std::path::Path::new(&file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let is_supported = matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
    );
    if !is_supported {
        return Err(AppError::NotFound("thumbnail not available".into()));
    }

    // Download the file content
    let content = Downloader::download_file(
        state.db.as_ref(),
        &repo_id,
        &normalized_path,
        &state.block_store,
        None,
    )
    .await
    .map_err(|_| AppError::NotFound("thumbnail not available".into()))?;

    // Generate thumbnail using the image crate
    let thumbnail_data = generate_thumbnail(&content, size)?;

    // Store thumbnail for future requests
    let thumbnail_dir = state
        .block_dir
        .parent()
        .unwrap_or(&state.block_dir)
        .join("thumbnails")
        .join(&repo_id);
    tokio::fs::create_dir_all(&thumbnail_dir).await?;
    let thumbnail_file = thumbnail_dir.join(format!(
        "{}_{}.png",
        normalize_path_for_file(&normalized_path),
        size
    ));
    tokio::fs::write(&thumbnail_file, &thumbnail_data).await?;

    // Record in database
    let now = chrono::Utc::now().timestamp();
    select_or_create_thumbnail(
        state.db.as_ref(),
        &repo_id,
        &normalized_path,
        size as i32,
        now,
    )
    .await?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/png")],
        thumbnail_data,
    )
        .into_response())
}

/// Generate a thumbnail image from raw file content.
fn generate_thumbnail(content: &[u8], size: u32) -> Result<Vec<u8>, AppError> {
    let img = image::load_from_memory(content)
        .map_err(|_| AppError::NotFound("unable to decode image".into()))?;

    let thumbnail = img.thumbnail(size, size);
    let mut output = Vec::new();
    thumbnail
        .write_to(
            &mut std::io::Cursor::new(&mut output),
            image::ImageFormat::Png,
        )
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(output)
}

/// Replace path separators with underscores for safe filenames.
fn normalize_path_for_file(path: &str) -> String {
    path.trim_matches('/').replace('/', "_")
}

/// Insert a thumbnail record if one doesn't already exist.
async fn select_or_create_thumbnail(
    db: &sea_orm::DatabaseConnection,
    repo_id: &str,
    path: &str,
    size: i32,
    now: i64,
) -> Result<(), AppError> {
    let existing = thumbnail_entity::Entity::find()
        .filter(thumbnail_entity::Column::RepoId.eq(repo_id))
        .filter(thumbnail_entity::Column::Path.eq(path))
        .filter(thumbnail_entity::Column::Size.eq(size))
        .one(db)
        .await?;

    if existing.is_none() {
        thumbnail_entity::Entity::insert(thumbnail_entity::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: sea_orm::Set(repo_id.to_string()),
            path: sea_orm::Set(path.to_string()),
            size: sea_orm::Set(size),
            file_modified_at: sea_orm::Set(now),
            created_at: sea_orm::Set(now),
        })
        .exec(db)
        .await?;
    }

    Ok(())
}

pub fn thumbnail_routes() -> Router<Arc<AppState>> {
    Router::new().route("/{repo_id}/thumbnail/", axum::routing::get(get_thumbnail))
}
