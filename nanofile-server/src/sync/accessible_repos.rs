use axum::{Json, Router, extract::State};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::auth::token::generate_sync_token;
use crate::entity::{repo, repo_member, sync_token};
use crate::error::AppError;

#[derive(Serialize)]
pub struct AccessibleRepo {
    pub repo_id: String,
    pub repo_name: String,
    pub repo_desc: String,
    pub owner_email: String,
    pub token: String,
    pub permission: String,
}

/// `GET /seafhttp/accessible-repos`
///
/// Returns all repos accessible to the authenticated user,
/// each with a sync token and permission info.
pub async fn accessible_repos(
    _auth: SyncAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<AccessibleRepo>>, AppError> {
    let memberships = repo_member::Entity::find()
        .filter(repo_member::Column::UserId.eq(_auth.user_id))
        .all(state.db.as_ref())
        .await?;

    let mut result = Vec::new();
    for member in &memberships {
        let r = repo::Entity::find_by_id(&member.repo_id)
            .one(state.db.as_ref())
            .await?;

        if let Some(r) = r {
            // Get or create sync token
            let token = get_or_create_sync_token(state.db.as_ref(), &r.id, _auth.user_id).await?;

            // Get owner email
            let owner = crate::entity::user::Entity::find_by_id(r.owner_id)
                .one(state.db.as_ref())
                .await?;

            result.push(AccessibleRepo {
                repo_id: r.id.clone(),
                repo_name: r.name,
                repo_desc: r.description,
                owner_email: owner.map(|u| u.email).unwrap_or_default(),
                token,
                permission: member.permission.clone(),
            });
        }
    }

    Ok(Json(result))
}

async fn get_or_create_sync_token(
    db: &sea_orm::DatabaseConnection,
    repo_id: &str,
    user_id: i32,
) -> Result<String, AppError> {
    // Check if token already exists
    if let Some(existing) = sync_token::Entity::find()
        .filter(sync_token::Column::RepoId.eq(repo_id))
        .filter(sync_token::Column::UserId.eq(user_id))
        .one(db)
        .await?
    {
        return Ok(existing.token);
    }

    let token_value = generate_sync_token();
    let now = chrono::Utc::now().timestamp();
    sync_token::Entity::insert(sync_token::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(repo_id.to_string()),
        user_id: sea_orm::Set(user_id),
        token: sea_orm::Set(token_value.clone()),
        created_at: sea_orm::Set(now),
        expires_at: sea_orm::Set(None),
        peer_id: sea_orm::NotSet,
        peer_name: sea_orm::NotSet,
        peer_ip: sea_orm::NotSet,
        client_version: sea_orm::NotSet,
        last_sync_time: sea_orm::NotSet,
    })
    .exec(db)
    .await?;

    Ok(token_value)
}

pub fn accessible_repos_routes() -> Router<Arc<AppState>> {
    Router::new().route("/accessible-repos", axum::routing::get(accessible_repos))
}
