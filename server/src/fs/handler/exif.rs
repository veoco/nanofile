use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::sanitize::safe_normalize_path;

#[derive(Deserialize)]
pub struct ExifQuery {
    pub p: Option<String>,
}

/// GET /api2/repos/{repo_id}/file/exif/?p=/path
///
/// Return EXIF metadata for an image file as a JSON object.
/// Returns `null` if the file contains no EXIF data.
pub async fn get_exif(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<ExifQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let path = query
        .p
        .ok_or_else(|| AppError::BadRequest("path is required".into()))?;
    let path = safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.exif_service();
    let result = svc.get_exif(&repo_id, &path).await?;

    Ok(Json(result))
}
