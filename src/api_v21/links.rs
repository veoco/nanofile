use axum::{
    Json,
    extract::{Path, State},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::auth::token::generate_share_link_token;
use crate::entity::{repo, share_link, upload_link};
use crate::error::AppError;

#[derive(Deserialize)]
pub struct CreateLinkRequest {
    pub repo_id: String,
    pub path: String,
    pub password: Option<String>,
    pub expire_days: Option<i64>,
}

/// GET /api/v2.1/share-links/
pub async fn list_share_links_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let links = share_link::Entity::find()
        .filter(share_link::Column::CreatorId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;

    let items: Vec<serde_json::Value> = links
        .into_iter()
        .map(|l| {
            serde_json::json!({
                "token": l.token,
                "repo_id": l.repo_id,
                "path": l.path,
                "created_at": l.created_at,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({"share_link_list": items})))
}

/// POST /api/v2.1/share-links/
pub async fn create_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateLinkRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Block share links for encrypted repos
    let repo_model = repo::Entity::find_by_id(&req.repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    if repo_model.encrypted != 0 {
        return Err(AppError::BadRequest(
            "cannot create share link for encrypted library".into(),
        ));
    }

    let token = generate_share_link_token();
    let now = chrono::Utc::now().timestamp();

    share_link::Entity::insert(share_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(req.repo_id.clone()),
        creator_id: Set(auth.user_id),
        path: Set(req.path.clone()),
        token: Set(token.clone()),
        password: Set(req.password.map(|p| sha256_hash(&p))),
        expires_at: Set(req.expire_days.map(|d| now + d * 86400)),
        created_at: Set(now),
    })
    .exec(state.db.as_ref())
    .await?;

    Ok(Json(serde_json::json!({"token": token})))
}

/// DELETE /api/v2.1/share-links/{token}/
pub async fn delete_share_link_v21(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    share_link::Entity::delete_many()
        .filter(share_link::Column::Token.eq(&token))
        .exec(state.db.as_ref())
        .await?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v2.1/upload-links/
pub async fn list_upload_links_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let links = upload_link::Entity::find()
        .filter(upload_link::Column::CreatorId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;

    let items: Vec<serde_json::Value> = links
        .into_iter()
        .map(|l| serde_json::json!({"token": l.token, "repo_id": l.repo_id, "path": l.path}))
        .collect();

    Ok(Json(serde_json::json!({"upload_link_list": items})))
}

/// POST /api/v2.1/upload-links/
pub async fn create_upload_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateLinkRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = generate_share_link_token();
    let now = chrono::Utc::now().timestamp();

    upload_link::Entity::insert(upload_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(req.repo_id),
        creator_id: Set(auth.user_id),
        path: Set(req.path),
        token: Set(token.clone()),
        password: Set(None),
        expires_at: Set(req.expire_days.map(|d| now + d * 86400)),
        created_at: Set(now),
    })
    .exec(state.db.as_ref())
    .await?;

    Ok(Json(serde_json::json!({"token": token})))
}

/// DELETE /api/v2.1/upload-links/{id}/
///
/// Returns bare `true` (not a JSON object) because the Android client's
/// DialogService.deleteUploadLink() uses `Single<Boolean>` and the
/// SupportResponseConverter's TypeAdapter<Boolean> cannot parse an object.
pub async fn delete_upload_link_v21(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    upload_link::Entity::delete_by_id(id)
        .exec(state.db.as_ref())
        .await?;
    Ok(Json(serde_json::Value::Bool(true)))
}

fn sha256_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}
