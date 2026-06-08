use axum::{
    RequestPartsExt,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::AppState;
use crate::entity::{api_token, sync_token, user};

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i32,
    pub email: String,
}

#[derive(Debug, Clone)]
pub struct SyncAuth {
    pub user_id: i32,
    pub repo_id: String,
}

impl FromRequestParts<std::sync::Arc<AppState>> for SyncAuth {
    type Rejection = crate::error::AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &std::sync::Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_sync_token(&parts.headers)?;
        let db = state.db.as_ref();

        // First try to authenticate via sync token (primary sync protocol path).
        // We look up sync_tokens directly here (rather than delegating to from_token)
        // so we can capture client_id from the request URI and store peer info.
        let sync_record = sync_token::Entity::find()
            .filter(sync_token::Column::Token.eq(&token))
            .one(db)
            .await
            .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;

        if let Some(record) = sync_record {
            // Check token expiry.
            if let Some(expires_at) = record.expires_at {
                let now = chrono::Utc::now().timestamp();
                if now > expires_at {
                    return Err(crate::error::AppError::Unauthorized);
                }
            }

            let user_id = record.user_id;
            let repo_id = record.repo_id.clone();

            // Capture client_id, client_name, client_ver from URL query params
            // and update the sync_token's peer info. This mirrors seafile-server's
            // RepoTokenPeerInfo table for device linking.
            if let Some(query) = parts.uri.query()
                && let Ok(params) =
                    serde_urlencoded::from_str::<std::collections::HashMap<String, String>>(query)
                && let Some(client_id) = params.get("client_id")
            {
                let now = chrono::Utc::now().timestamp();
                let mut active: sync_token::ActiveModel = record.into();
                active.peer_id = sea_orm::Set(Some(client_id.clone()));
                active.peer_name = sea_orm::Set(params.get("client_name").cloned());
                active.client_version = sea_orm::Set(params.get("client_ver").cloned());
                // Try to capture client IP from common proxy headers.
                active.peer_ip = sea_orm::Set(
                    parts
                        .headers
                        .get("x-forwarded-for")
                        .and_then(|v| v.to_str().ok())
                        .or_else(|| {
                            parts.headers.get("x-real-ip").and_then(|v| v.to_str().ok())
                        })
                        .map(|s| s.to_string()),
                );
                active.last_sync_time = sea_orm::Set(Some(now));
                let _ = active.update(db).await;
            }

            return Ok(SyncAuth { user_id, repo_id });
        }

        // Fall back to API token (for requests using Bearer/Token auth).
        SyncAuth::from_token(db, &token)
            .await
            .map_err(|_| crate::error::AppError::Unauthorized)
    }
}

impl FromRequestParts<std::sync::Arc<AppState>> for AuthUser {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &std::sync::Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token_str = if let Ok(TypedHeader(Authorization(bearer))) =
            parts.extract::<TypedHeader<Authorization<Bearer>>>().await
        {
            bearer.token().to_string()
        } else if let Some(auth) = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
        {
            if let Some(token) = auth.strip_prefix("Token ") {
                token.to_string()
            } else if let Some(token) = auth.strip_prefix("Bearer ") {
                token.to_string()
            } else {
                return Err(StatusCode::UNAUTHORIZED);
            }
        } else {
            return Err(StatusCode::UNAUTHORIZED);
        };

        let db = state.db.as_ref();

        let mut user_id: Option<i32> = None;

        // Try API token first
        if let Ok(Some(token_record)) = api_token::Entity::find()
            .filter(api_token::Column::Token.eq(&token_str))
            .one(db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        {
            user_id = Some(token_record.user_id);
        }

        // Fall back to sync token (seaf-cli calls download-info with sync token)
        if user_id.is_none()
            && let Ok(Some(sync_record)) = sync_token::Entity::find()
                .filter(sync_token::Column::Token.eq(&token_str))
                .one(db)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        {
            if let Some(expires_at) = sync_record.expires_at {
                let now = chrono::Utc::now().timestamp();
                if now > expires_at {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }
            user_id = Some(sync_record.user_id);
        }

        let user_id = user_id.ok_or(StatusCode::UNAUTHORIZED)?;

        let user_record = user::Entity::find_by_id(user_id)
            .one(db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::UNAUTHORIZED)?;

        if !user_record.is_active {
            return Err(StatusCode::FORBIDDEN);
        }

        Ok(AuthUser {
            user_id: user_record.id,
            email: user_record.email,
        })
    }
}

impl SyncAuth {
    /// Authenticate via sync token or API token.
    pub async fn from_token(db: &DatabaseConnection, token_str: &str) -> Result<Self, StatusCode> {
        // First try sync token (has repo_id associated)
        if let Ok(Some(record)) = sync_token::Entity::find()
            .filter(sync_token::Column::Token.eq(token_str))
            .one(db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        {
            if let Some(expires_at) = record.expires_at {
                let now = chrono::Utc::now().timestamp();
                if now > expires_at {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }

            return Ok(SyncAuth {
                user_id: record.user_id,
                repo_id: record.repo_id,
            });
        }

        // Fall back to API token (no expiry — matches seahub behavior)
        if let Ok(Some(record)) = api_token::Entity::find()
            .filter(api_token::Column::Token.eq(token_str))
            .one(db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        {
            return Ok(SyncAuth {
                user_id: record.user_id,
                repo_id: String::new(),
            });
        }

        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Extract a sync/API token from HTTP headers. Used by /seafhttp/ endpoints.
/// Checks the Seafile-Repo-Token header first, then the Authorization header.
pub fn extract_sync_token(
    headers: &axum::http::HeaderMap,
) -> Result<String, crate::error::AppError> {
    use crate::error::AppError;

    if let Some(token) = headers
        .get("Seafile-Repo-Token")
        .and_then(|v| v.to_str().ok())
    {
        return Ok(token.to_string());
    }

    if let Some(auth) = headers.get("Authorization").and_then(|v| v.to_str().ok())
        && let Some(token) = auth.strip_prefix("Token ")
    {
        return Ok(token.to_string());
    }

    Err(AppError::Unauthorized)
}
