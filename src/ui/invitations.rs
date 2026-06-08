/// Web UI invitation code management handlers — admin only.
///
/// GET  /profile/invitations/ — list invitation codes.
/// POST /profile/invitations/ — generate a new invitation code.
/// POST /profile/invitations/{id}/delete — delete an existing invitation code.
use askama::Template;
use axum::{
    extract::{Form, Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::entity::invitation_code;
use crate::entity::user;
use crate::error::AppError;

use super::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "settings/invitations.html")]
pub struct InvitationsTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub active_page: &'static str,
    pub invitations: Vec<InvitationInfo>,
    pub error: Option<String>,
    pub success: Option<String>,
}

pub struct InvitationInfo {
    pub code: String,
    pub bound_email: Option<String>,
    pub created_at: i64,
    pub used_by_email: Option<String>,
    pub used_at: Option<i64>,
    pub id: i32,
}

#[derive(Deserialize)]
pub struct GenerateForm {
    pub email: Option<String>,
}

/// GET /profile/invitations/ — list invitation codes (admin only).
pub async fn list_invitations(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    let db = state.db.as_ref();

    let codes = invitation_code::Entity::find()
        .filter(invitation_code::Column::CreatorId.eq(user.user_id))
        .order_by_desc(invitation_code::Column::CreatedAt)
        .all(db)
        .await?;

    let mut invitations = Vec::with_capacity(codes.len());
    for code in codes {
        let used_by_email = if let Some(uid) = code.used_by {
            user::Entity::find_by_id(uid)
                .one(db)
                .await?
                .map(|u| u.email)
        } else {
            None
        };

        invitations.push(InvitationInfo {
            id: code.id,
            code: code.code,
            bound_email: code.email,
            created_at: code.created_at,
            used_by_email,
            used_at: code.used_at,
        });
    }

    let tpl = InvitationsTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        is_admin: user.is_admin,
        active_page: "settings",
        invitations,
        error: None,
        success: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /profile/invitations/ — create a new invitation code (admin only).
pub async fn generate_invitation(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<GenerateForm>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    let db = state.db.as_ref();

    let code_str = invitation_code::generate_invitation_code();
    let now = chrono::Utc::now().timestamp();

    // Trim and validate email if provided.
    let email = form
        .email
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty());

    let model = invitation_code::ActiveModel {
        id: sea_orm::NotSet,
        code: Set(code_str),
        email: Set(email),
        creator_id: Set(user.user_id),
        created_at: Set(now),
        used_by: Set(None),
        used_at: Set(None),
    };

    model.insert(db).await?;

    Ok((StatusCode::FOUND, [("Location", "/profile/invitations/")]).into_response())
}

/// POST /profile/invitations/{id}/delete — remove an invitation code (admin only).
pub async fn delete_invitation(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    let db = state.db.as_ref();

    // Only allow deletion of codes owned by the current admin.
    let result = invitation_code::Entity::delete_many()
        .filter(invitation_code::Column::Id.eq(id))
        .filter(invitation_code::Column::CreatorId.eq(user.user_id))
        .exec(db)
        .await?;

    if result.rows_affected == 0 {
        return Err(AppError::NotFound("Invitation code not found.".to_string()));
    }

    Ok((StatusCode::FOUND, [("Location", "/profile/invitations/")]).into_response())
}
