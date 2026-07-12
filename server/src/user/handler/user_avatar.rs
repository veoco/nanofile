use axum::{
    Json,
    extract::{Multipart, State},
};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::user::service::AvatarService;

// ─── Upload handler ──────────────────────────────────────────────────────────

/// `POST /api/v2.1/user-avatar/`
///
/// Upload a new primary avatar for the authenticated user.
///
/// Request (multipart/form-data):
///   - `avatar` — the image file
///
/// Response (JSON):
///   ```json
///   { "avatar_url": "/avatars/user/{email}/resized/256/" }
///   ```
pub async fn upload_avatar(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    // Parse the multipart stream and find the "avatar" file field.
    let mut avatar_field: Option<(String, Vec<u8>, String)> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "avatar" || name == "file" {
            let file_name = field.file_name().unwrap_or("avatar.png").to_string();
            let content_type = field
                .content_type()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "image/png".to_string());
            let data = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("read error: {e}")))?
                .to_vec();

            avatar_field = Some((file_name, data, content_type));
            break; // Only process the first avatar field
        }
    }

    let (file_name, data, _content_type) =
        avatar_field.ok_or_else(|| AppError::BadRequest("no avatar file provided".into()))?;

    let svc = AvatarService::new(state.repos.clone());
    let avatar_url = svc.upload_avatar(&auth.email, file_name, data).await?;

    Ok(Json(serde_json::json!({
        "avatar_url": avatar_url
    })))
}
