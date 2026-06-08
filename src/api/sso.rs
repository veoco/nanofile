use axum::Json;
use axum::extract::{Form, Path, State};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::sso_login_token;
use crate::error::AppError;

/// POST /api2/client-login/
pub async fn client_login(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    sso_login_token::Entity::insert(sso_login_token::ActiveModel {
        id: sea_orm::NotSet,
        token: Set(token.clone()),
        platform: Set(None),
        device_id: Set(None),
        device_name: Set(None),
        status: Set("pending".to_string()),
        username: Set(None),
        api_token: Set(None),
        created_at: Set(now),
        expires_at: Set(Some(now + 3600)),
    })
    .exec(state.db.as_ref())
    .await?;

    Ok(Json(serde_json::json!({"token": token})))
}

/// POST /api2/client-sso-link/
pub async fn client_sso_link(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    sso_login_token::Entity::insert(sso_login_token::ActiveModel {
        id: sea_orm::NotSet,
        token: Set(token.clone()),
        platform: Set(form.get("platform").cloned()),
        device_id: Set(form.get("device_id").cloned()),
        device_name: Set(form.get("device_name").cloned()),
        status: Set("pending".to_string()),
        username: Set(None),
        api_token: Set(None),
        created_at: Set(now),
        expires_at: Set(Some(now + 3600)),
    })
    .exec(state.db.as_ref())
    .await?;

    let link = format!("/api2/client-sso-link/{}/", token);

    Ok(Json(serde_json::json!({
        "link": link,
        "token": token,
    })))
}

/// GET /api2/client-sso-link/{token}/
pub async fn poll_sso_link(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let record = sso_login_token::Entity::find()
        .filter(sso_login_token::Column::Token.eq(&token))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("token not found".into()))?;

    if record.status == "done" {
        Ok(Json(serde_json::json!({
            "status": "done",
            "api_token": record.api_token,
        })))
    } else {
        Ok(Json(serde_json::json!({
            "status": "pending",
        })))
    }
}
