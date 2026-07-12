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
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::service::user::{InvitationInfo, InvitationService};
use crate::ui::auth_extractor::WebUser;
use base::error::AppError;

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
    pub csrf_token: String,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

#[derive(Deserialize)]
pub struct GenerateForm {
    pub email: Option<String>,
    pub csrf_token: Option<String>,
}

/// GET /profile/invitations/ — list invitation codes (admin only).
pub async fn list_invitations(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    let svc = InvitationService::new(state.repos.clone());
    let invitations = svc.list_invitations(user.user_id).await?;

    let csrf_token =
        crate::service::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);

    let left_panel_repos =
        crate::service::repo::service::load_left_panel_repos(&state.repos, user.user_id).await?;
    let tpl = InvitationsTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        is_admin: user.is_admin,
        active_page: "settings",
        invitations,
        error: None,
        success: None,
        csrf_token,
        left_panel_repos,
        current_repo_id: None,
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
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    let svc = InvitationService::new(state.repos.clone());
    svc.generate_invitation(user.user_id, form.email).await?;

    Ok((StatusCode::FOUND, [("Location", "/profile/invitations/")]).into_response())
}

/// POST /profile/invitations/{id}/delete — remove an invitation code (admin only).
pub async fn delete_invitation(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    let svc = InvitationService::new(state.repos.clone());
    svc.delete_invitation(user.user_id, id).await?;

    Ok((StatusCode::FOUND, [("Location", "/profile/invitations/")]).into_response())
}
