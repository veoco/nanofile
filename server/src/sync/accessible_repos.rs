use axum::{Json, Router, extract::State};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::auth::token::generate_sync_token;
use base::error::AppError;

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
    let memberships = state.repos.member.find_by_user_id(_auth.user_id).await?;

    let mut result = Vec::new();
    for member in &memberships {
        let r = state.repos.repo.find_by_id(&member.repo_id).await?;

        if let Some(r) = r {
            // Get or create sync token
            let token = get_or_create_sync_token(&state, &r.id, _auth.user_id).await?;

            // Get owner email
            let owner = state.repos.user.find_by_id(r.owner_id).await?;

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
    state: &Arc<AppState>,
    repo_id: &str,
    user_id: i32,
) -> Result<String, AppError> {
    // Check if token already exists
    if let Some(existing) = state
        .repos
        .sync_token
        .find_by_repo_and_user(repo_id, user_id)
        .await?
    {
        return Ok(existing.token);
    }

    let token_value = generate_sync_token();
    let now = chrono::Utc::now().timestamp();
    state
        .repos
        .sync_token
        .create(repo_id, user_id, token_value.clone(), None, now)
        .await?;

    Ok(token_value)
}

pub fn accessible_repos_routes() -> Router<Arc<AppState>> {
    Router::new().route("/accessible-repos", axum::routing::get(accessible_repos))
}
