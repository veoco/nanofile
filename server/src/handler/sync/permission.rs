use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::SyncAuth;
use base::error::AppError;

#[derive(Deserialize)]
pub struct PermissionQuery {
    op: Option<String>,
    /// 40-hex client id; upstream rejects any other length (`client_ver` is also sent
    /// but its version gate is not enforced here).
    client_id: Option<String>,
}

pub fn permission_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/{repo_id}/permission-check/",
        axum::routing::get(permission_check),
    )
}

/// seafile-server answers 200 with an empty body on success, 403 for no permission, 444
/// for a deleted repo and 445 for a corrupted one. "upload" ops require write (rw)
/// permission, "download" ops read (r). On a 403 the daemon parses the body's `reason`
/// field to tell "no write permission" from a generic access-denied.
pub async fn permission_check(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    Query(query): Query<PermissionQuery>,
) -> Result<Response, AppError> {
    if !state.sync_service().repo_exists(&repo_id).await? {
        return Err(AppError::RepoDeleted);
    }

    // Upstream validates the op and the optional client id before any permission
    // work (`fileserver/sync_api.go:197-216`).
    match query.op.as_deref() {
        Some("upload") | Some("download") => {}
        _ => return Err(AppError::BadRequest("op is invalid".into())),
    }
    if let Some(client_id) = query.client_id.as_deref()
        && !client_id.is_empty()
        && client_id.len() != 40
    {
        return Err(AppError::BadRequest("client_id is invalid".into()));
    }

    match query.op.as_deref() {
        Some("upload") => {
            match crate::domain::permission::check_repo_write_permission(
                state.repos.member.as_ref(),
                &repo_id,
                _auth.user_id,
            )
            .await
            {
                Ok(()) => {}
                // Read-only members (and any non-write access) are reported to
                // the daemon as "no write permission" rather than a generic 403.
                Err(AppError::Forbidden) => {
                    return Ok((
                        StatusCode::FORBIDDEN,
                        Json(serde_json::json!({ "reason": "no write permission" })),
                    )
                        .into_response());
                }
                Err(e) => return Err(e),
            }
        }
        _ => {
            // op == "download" (validated above).
            crate::domain::permission::check_repo_read_permission(
                state.repos.member.as_ref(),
                &repo_id,
                _auth.user_id,
            )
            .await?;
        }
    }

    Ok(StatusCode::OK.into_response())
}
