/// Web UI share handlers — list, create, delete share links.
use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;
use std::sync::Arc;

use chrono::TimeZone;

use crate::AppState;
use crate::auth::token::generate_share_link_token;
use crate::entity::share_link;
use crate::error::AppError;

use super::auth_extractor::WebUser;

fn format_ts(ts: i64) -> String {
    chrono::Utc
        .timestamp_opt(ts, 0)
        .unwrap()
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn format_ts_opt(ts: Option<i64>) -> String {
    match ts {
        Some(t) => format_ts(t),
        None => "Never".to_string(),
    }
}

#[derive(Template)]
#[template(path = "shares/list.html")]
pub struct SharesTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: String,
    pub share_links: Vec<ShareLinkInfo>,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::repo::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

pub struct ShareLinkInfo {
    pub token: String,
    pub repo_id: String,
    pub path: String,
    pub name: String,
    pub created_at: String,
    pub expires_at: String,
    pub has_password: bool,
    pub view_cnt: i64,
    pub s_type: String,
    pub link_url: String,
}

#[derive(Deserialize)]
pub struct CreateShareForm {
    pub repo_id: String,
    pub path: String,
    pub csrf_token: Option<String>,
}

/// GET /share/ — list all share links.
pub async fn list_shares(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let db = state.db.as_ref();

    let links = share_link::Entity::find()
        .filter(share_link::Column::CreatorId.eq(user.user_id))
        .all(db)
        .await
        .map_err(|e| AppError::internal(format!("db error: {e}")))?;

    let share_links: Vec<ShareLinkInfo> = links
        .into_iter()
        .map(|s| {
            let name = s
                .path
                .rsplit_once('/')
                .map(|(_, n)| n.to_string())
                .unwrap_or_else(|| s.path.clone());
            ShareLinkInfo {
                token: s.token.clone(),
                repo_id: s.repo_id,
                path: s.path.clone(),
                name,
                created_at: format_ts(s.created_at),
                expires_at: format_ts_opt(s.expires_at),
                has_password: s.password.is_some(),
                view_cnt: s.view_cnt,
                s_type: s.s_type.clone(),
                link_url: if s.s_type == "d" {
                    format!("/d/{}/", s.token)
                } else {
                    format!("/f/{}/", s.token)
                },
            }
        })
        .collect();

    let csrf_token =
        crate::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
    let left_panel_repos =
        crate::repo::load_left_panel_repos(state.db.as_ref(), user.user_id).await?;
    let tpl = SharesTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        is_admin: user.is_admin,
        csrf_token,
        share_links,
        active_page: "shares",
        left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /share/create — create a new share link.
pub async fn create_share(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    axum::Form(form): axum::Form<CreateShareForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::auth::csrf::check_form_csrf(&state, &user.session_token, form.csrf_token.as_deref())?;
    let db = state.db.as_ref();
    let now = chrono::Utc::now().timestamp();

    // Determine s_type by walking the FS tree
    let s_type =
        crate::sharing::service::share::resolve_entry_type_raw(db, &form.repo_id, &form.path)
            .await
            .unwrap_or_else(|_| "f".to_string());

    let link = share_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(form.repo_id),
        creator_id: Set(user.user_id),
        path: Set(form.path),
        token: Set(generate_share_link_token()),
        password: Set(None),
        expires_at: Set(None),
        created_at: Set(now),
        s_type: Set(s_type),
        view_cnt: Set(0i64),
        description: Set(None),
    };

    link.insert(db)
        .await
        .map_err(|e| AppError::internal(format!("create share failed: {e}")))?;
    Ok((StatusCode::FOUND, [("Location", "/shares/")]).into_response())
}

/// POST /share/{token}/delete — delete a share link.
pub async fn delete_share(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    crate::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;
    let db = state.db.as_ref();

    let link = share_link::Entity::find()
        .filter(share_link::Column::Token.eq(&token))
        .one(db)
        .await
        .map_err(|e| AppError::internal(format!("db error: {e}")))?
        .ok_or_else(|| AppError::NotFound("Share link not found".to_string()))?;

    if link.creator_id != user.user_id {
        return Err(AppError::Forbidden);
    }

    share_link::Entity::delete_many()
        .filter(share_link::Column::Token.eq(&token))
        .exec(db)
        .await
        .map_err(|e| AppError::internal(format!("delete failed: {e}")))?;

    Ok((StatusCode::FOUND, [("Location", "/shares/")]).into_response())
}
