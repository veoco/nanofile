/// Web UI file activities page.
use askama::Template;
use axum::{extract::State, response::Html};
use chrono::DateTime;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use base::error::AppError;

use super::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "activities/list.html")]
pub struct ActivitiesTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub activities: Vec<ActivityView>,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::repo::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

pub struct ActivityView {
    pub op_type: String,
    pub obj_type: String,
    pub repo_id: String,
    pub repo_name: String,
    pub path: String,
    pub name: String,
    pub old_path: Option<String>,
    pub old_path_display: String,
    pub author_email: String,
    pub time_display: String,
    pub time_iso: String,
    /// Number of items in a batch operation (1 for single operations).
    pub batch_count: usize,
    /// File names extracted from detail JSON (empty for single operations).
    pub detail_items: Vec<String>,
    /// Old repo name for repo rename operations.
    pub old_repo_name: Option<String>,
}

impl ActivityView {
    pub fn has_old_path(&self) -> bool {
        self.old_path.is_some()
    }
}

/// GET /activities/ — list file activity history.
pub async fn activities_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    // Fetch latest 50 activities for this user
    let events = state
        .repos
        .activity
        .find_recent_by_user(user.user_id, 50)
        .await?;

    // Batch-load repo names
    let mut repo_cache: HashMap<String, Option<String>> = HashMap::new();
    for e in &events {
        #[allow(clippy::map_entry)]
        if !repo_cache.contains_key(&e.repo_id) {
            let r = state.repos.repo.find_by_id(&e.repo_id).await?;
            repo_cache.insert(e.repo_id.clone(), r.map(|r| r.name));
        }
    }

    // Batch-load user emails
    let mut user_cache: HashMap<i32, Option<String>> = HashMap::new();
    for e in &events {
        #[allow(clippy::map_entry)]
        if !user_cache.contains_key(&e.user_id) {
            let u = state.repos.user.find_by_id(e.user_id).await?;
            user_cache.insert(e.user_id, u.map(|u| u.email));
        }
    }

    let mut activities = Vec::with_capacity(events.len());

    for e in &events {
        let repo_name = repo_cache
            .get(&e.repo_id)
            .cloned()
            .flatten()
            .unwrap_or_default();

        let email = user_cache
            .get(&e.user_id)
            .cloned()
            .flatten()
            .unwrap_or_default();

        let name = if e.obj_type == "repo" {
            repo_name.clone()
        } else {
            e.path
                .rsplit_once('/')
                .map(|(_, n)| n.to_string())
                .unwrap_or_default()
        };

        let formatted = super::files::format_mtime(e.created_at);

        let time_iso = DateTime::from_timestamp(e.created_at, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();

        let old_path_display = e.old_path.as_deref().unwrap_or("").to_string();

        // Parse detail JSON for batch item names and repo rename info.
        let (batch_count, detail_items, old_repo_name) =
            match serde_json::from_str::<serde_json::Value>(&e.detail) {
                Ok(serde_json::Value::Array(arr)) => {
                    let items: Vec<String> = arr
                        .iter()
                        .filter_map(|d| d.get("path").and_then(|p| p.as_str()))
                        .map(|p| {
                            p.rsplit_once('/')
                                .map(|(_, n)| n.to_string())
                                .unwrap_or_else(|| p.to_string())
                        })
                        .collect();
                    let count = items.len();
                    let orn = arr
                        .first()
                        .and_then(|d| d.get("old_repo_name"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    (count, items, orn)
                }
                Ok(serde_json::Value::Object(obj)) => {
                    let orn = obj
                        .get("old_repo_name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    (1, vec![], orn)
                }
                _ => (1, vec![], None),
            };

        activities.push(ActivityView {
            op_type: e.op_type.clone(),
            obj_type: e.obj_type.clone(),
            repo_id: e.repo_id.clone(),
            repo_name,
            path: e.path.clone(),
            name,
            old_path: e.old_path.clone(),
            old_path_display,
            author_email: email,
            time_display: formatted,
            time_iso,
            batch_count,
            detail_items,
            old_repo_name,
        });
    }

    let tpl = ActivitiesTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        is_admin: user.is_admin,
        activities,
        active_page: "activities",
        left_panel_repos: crate::repo::load_left_panel_repos(&state.repos, user.user_id).await?,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}
