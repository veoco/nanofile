use axum::{
    Json, Router,
    extract::{Path, State},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::auth::token::generate_upload_link_token;
use crate::entity::upload_link;
use crate::error::AppError;

#[derive(Deserialize)]
pub struct CreateUploadLinkRequest {
    pub repo_id: String,
    pub path: String,
    pub password: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Serialize)]
pub struct UploadLinkInfo {
    pub token: String,
    pub link: String,
    pub repo_id: String,
    pub path: String,
    pub created_at: i64,
}

pub fn upload_link_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/",
            axum::routing::get(list_upload_links).post(create_upload_link),
        )
        .route("/{token}", axum::routing::delete(delete_upload_link))
}

pub async fn list_upload_links(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<UploadLinkInfo>>, AppError> {
    let links = upload_link::Entity::find()
        .filter(upload_link::Column::CreatorId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;

    let infos: Vec<UploadLinkInfo> = links
        .into_iter()
        .map(|l| UploadLinkInfo {
            token: l.token.clone(),
            link: format!("/u/{}/", l.token),
            repo_id: l.repo_id,
            path: l.path,
            created_at: l.created_at,
        })
        .collect();

    Ok(Json(infos))
}

pub async fn create_upload_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateUploadLinkRequest>,
) -> Result<Json<UploadLinkInfo>, AppError> {
    let token = generate_upload_link_token();
    let now = chrono::Utc::now().timestamp();

    let password_hash = req
        .password
        .map(|p| crate::auth::password::hash_password_legacy(&p));

    let model = upload_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(req.repo_id.clone()),
        creator_id: sea_orm::Set(auth.user_id),
        path: sea_orm::Set(req.path.clone()),
        token: sea_orm::Set(token.clone()),
        password: sea_orm::Set(password_hash),
        expires_at: sea_orm::Set(req.expires_at),
        created_at: sea_orm::Set(now),
    };
    upload_link::Entity::insert(model)
        .exec(state.db.as_ref())
        .await?;

    Ok(Json(UploadLinkInfo {
        token: token.clone(),
        link: format!("/u/{}/", token),
        repo_id: req.repo_id,
        path: req.path,
        created_at: now,
    }))
}

pub async fn delete_upload_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<(), AppError> {
    upload_link::Entity::delete_many()
        .filter(upload_link::Column::Token.eq(&token))
        .filter(upload_link::Column::CreatorId.eq(auth.user_id))
        .exec(state.db.as_ref())
        .await?;

    Ok(())
}
