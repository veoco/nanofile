/// Admin Web UI — share management (view/delete all share and upload links).
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use chrono::TimeZone;

use crate::AppState;
use base::error::AppError;

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
#[template(path = "adminshares/list.html")]
pub struct AdminSharesTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: Option<String>,
    pub share_links: Vec<AdminShareLinkInfo>,
    pub upload_links: Vec<AdminUploadLinkInfo>,
    pub active_page: &'static str,
    pub active_tab: String,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

pub struct AdminShareLinkInfo {
    pub token: String,
    pub repo_id: String,
    pub repo_name: String,
    pub path: String,
    pub name: String,
    pub creator_email: String,
    pub created_at: String,
    pub expires_at: String,
    pub has_password: bool,
    pub view_cnt: i64,
    pub s_type: String,
    pub link_url: String,
    pub description: Option<String>,
}

pub struct AdminUploadLinkInfo {
    pub token: String,
    pub repo_id: String,
    pub repo_name: String,
    pub path: String,
    pub name: String,
    pub creator_email: String,
    pub created_at: String,
    pub expires_at: String,
    pub has_password: bool,
    pub view_cnt: i64,
    pub link_url: String,
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct AdminSharesQuery {
    pub tab: Option<String>,
}

/// GET /sysadmin/shares/ — list all share and upload links (admin only).
pub async fn list_all_shares(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<AdminSharesQuery>,
) -> Response {
    if !user.is_admin {
        return Redirect::to("/libraries/").into_response();
    }

    let share_models = match state.repos.share_link.find_all().await {
        Ok(m) => m,
        Err(e) => return AppError::internal(format!("db error: {e}")).into_response(),
    };
    let upload_models = match state.repos.upload_link.find_all().await {
        Ok(m) => m,
        Err(e) => return AppError::internal(format!("db error: {e}")).into_response(),
    };

    // Build creator email lookup
    let mut creator_ids: Vec<i32> = Vec::new();
    for s in &share_models {
        if !creator_ids.contains(&s.creator_id) {
            creator_ids.push(s.creator_id);
        }
    }
    for u in &upload_models {
        if !creator_ids.contains(&u.creator_id) {
            creator_ids.push(u.creator_id);
        }
    }
    let mut creator_emails: HashMap<i32, String> = HashMap::new();
    for cid in &creator_ids {
        let u = state.repos.user.find_by_id(*cid).await.unwrap_or(None);
        creator_emails.insert(*cid, u.map(|u| u.email).unwrap_or_default());
    }

    // Build repo name lookup
    let mut repo_ids: Vec<String> = Vec::new();
    for s in &share_models {
        if !repo_ids.contains(&s.repo_id) {
            repo_ids.push(s.repo_id.clone());
        }
    }
    for u in &upload_models {
        if !repo_ids.contains(&u.repo_id) {
            repo_ids.push(u.repo_id.clone());
        }
    }
    let mut repo_names: HashMap<String, String> = HashMap::new();
    for rid in &repo_ids {
        let r = state.repos.repo.find_by_id(rid).await.unwrap_or(None);
        repo_names.insert(rid.clone(), r.map(|r| r.name).unwrap_or_default());
    }

    let share_links: Vec<AdminShareLinkInfo> = share_models
        .into_iter()
        .map(|s| {
            let name = s
                .path
                .rsplit_once('/')
                .map(|(_, n)| n.to_string())
                .unwrap_or_else(|| s.path.clone());
            AdminShareLinkInfo {
                token: s.token.clone(),
                repo_id: s.repo_id.clone(),
                repo_name: repo_names.get(&s.repo_id).cloned().unwrap_or_default(),
                path: s.path.clone(),
                name,
                creator_email: creator_emails
                    .get(&s.creator_id)
                    .cloned()
                    .unwrap_or_default(),
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
                description: s.description,
            }
        })
        .collect();

    let upload_links: Vec<AdminUploadLinkInfo> = upload_models
        .into_iter()
        .map(|u| {
            let name = u
                .path
                .trim_end_matches('/')
                .rsplit_once('/')
                .map(|(_, n)| n.to_string())
                .unwrap_or_else(|| u.path.clone());
            AdminUploadLinkInfo {
                token: u.token.clone(),
                repo_id: u.repo_id.clone(),
                repo_name: repo_names.get(&u.repo_id).cloned().unwrap_or_default(),
                path: u.path.clone(),
                name,
                creator_email: creator_emails
                    .get(&u.creator_id)
                    .cloned()
                    .unwrap_or_default(),
                created_at: format_ts(u.created_at),
                expires_at: format_ts_opt(u.expires_at),
                has_password: u.password.is_some(),
                view_cnt: u.view_cnt,
                link_url: format!("/u/{}/", u.token),
                description: u.description,
            }
        })
        .collect();

    let active_tab = query
        .tab
        .filter(|t| t == "upload-links")
        .unwrap_or("share-links".to_string());

    let csrf_token = Some(crate::service::auth::csrf::generate_csrf_token(
        &state.csrf_secret,
        &user.session_token,
    ));

    let left_panel_repos = match crate::service::repo::service::load_left_panel_repos(
        &state.repos,
        user.user_id,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return AppError::internal(e.to_string()).into_response(),
    };

    let tpl = AdminSharesTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        is_admin: user.is_admin,
        csrf_token,
        share_links,
        upload_links,
        active_page: "adminshares",
        active_tab,
        left_panel_repos,
        current_repo_id: None,
    };

    match tpl.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => AppError::internal(e.to_string()).into_response(),
    }
}

/// POST /sysadmin/shares/share/{token}/delete/ — delete any share link (admin).
pub async fn delete_share(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;

    // Admin delete — no creator_id check.
    state.repos.share_link.delete_by_token(&token).await?;

    let redirect = match form.get("tab").map(|s| s.as_str()) {
        Some("upload-links") => "/sysadmin/shares/?tab=upload-links",
        _ => "/sysadmin/shares/",
    };
    Ok((StatusCode::FOUND, [("Location", redirect)]).into_response())
}

/// POST /sysadmin/shares/upload/{token}/delete/ — delete any upload link (admin).
pub async fn delete_upload(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;

    // Admin delete — no creator_id check.
    state.repos.upload_link.delete_by_token(&token).await?;

    let redirect = match form.get("tab").map(|s| s.as_str()) {
        Some("upload-links") => "/sysadmin/shares/?tab=upload-links",
        _ => "/sysadmin/shares/",
    };
    Ok((StatusCode::FOUND, [("Location", redirect)]).into_response())
}
