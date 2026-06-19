use axum::{
    Json,
    extract::{Multipart, State},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::avatar as avatar_entity;
use crate::error::AppError;

// ─── Constants ───────────────────────────────────────────────────────────────

const MAX_AVATAR_SIZE: usize = 1024 * 1024; // 1 MB
const ALLOWED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif"];

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
    let db = state.db.as_ref();

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

    // Validate file size
    if data.len() > MAX_AVATAR_SIZE {
        return Err(AppError::BadRequest(format!(
            "avatar file too large (max {} bytes)",
            MAX_AVATAR_SIZE
        )));
    }

    // Validate file extension
    let ext = file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .ok_or_else(|| AppError::BadRequest("file has no extension".into()))?;

    if !ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
        return Err(AppError::BadRequest(format!(
            "invalid file extension: .{ext} (allowed: {:?})",
            ALLOWED_EXTENSIONS
        )));
    }

    // Determine mime type from extension (match what seafile does)
    let mime_type = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        _ => "image/png",
    };

    // Build storage path
    let storage_dir = crate::api::avatar::avatar_storage_dir(&auth.email);
    tokio::fs::create_dir_all(&storage_dir)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create avatar dir: {e}")))?;

    // Save the original file
    let original_path = storage_dir.join(format!("original.{}", ext));
    tokio::fs::write(&original_path, &data)
        .await
        .map_err(|e| AppError::Internal(format!("failed to save avatar: {e}")))?;

    // Clean up stale thumbnails from any previous upload
    let _ = tokio::task::spawn_blocking({
        let dir = storage_dir.clone();
        move || {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    // Remove any cached {size}.png thumbnails
                    if name.ends_with(".png") && name != format!("original.{}", ext) {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    })
    .await;

    // Generate and save the default-size thumbnail (256×256)
    let thumbnail_data = generate_thumbnail(&data, 256)
        .map_err(|_| AppError::BadRequest("unable to decode image".into()))?;
    let thumbnail_path = storage_dir.join("256.png");
    let _ = tokio::fs::write(&thumbnail_path, &thumbnail_data).await;

    // Upsert avatar database record
    let now = chrono::Utc::now().timestamp();
    let existing = avatar_entity::Entity::find()
        .filter(avatar_entity::Column::Email.eq(&auth.email))
        .one(db)
        .await?;

    if let Some(record) = existing {
        // Update existing record
        let mut active: avatar_entity::ActiveModel = record.into();
        active.avatar_file_name = Set(file_name.clone());
        active.mime_type = Set(mime_type.to_string());
        active.file_size = Set(data.len() as i32);
        active.date_uploaded = Set(now);
        active.update(db).await?;
    } else {
        // Create new record
        avatar_entity::ActiveModel {
            id: sea_orm::NotSet,
            email: Set(auth.email.clone()),
            avatar_file_name: Set(file_name),
            mime_type: Set(mime_type.to_string()),
            file_size: Set(data.len() as i32),
            date_uploaded: Set(now),
        }
        .insert(db)
        .await?;
    }

    Ok(Json(serde_json::json!({
        "avatar_url": crate::api::avatar::primary_avatar_url(&auth.email, 256)
    })))
}

/// Generate a square PNG thumbnail from raw image data.
fn generate_thumbnail(content: &[u8], size: u32) -> Result<Vec<u8>, AppError> {
    let img = image::load_from_memory(content)
        .map_err(|_| AppError::BadRequest("unable to decode image".into()))?;

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
