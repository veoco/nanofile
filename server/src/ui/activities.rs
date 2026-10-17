/// Web UI file activities page.
use askama::Template;
use axum::{extract::State, response::Html};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use base::error::AppError;

use super::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "activities/list.html")]
pub struct ActivitiesTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub activities: Vec<ActivityView>,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

pub struct ActivityView {
    pub op_type: String,
    pub obj_type: String,
    pub repo_id: String,
    pub repo_name: String,
    pub path: String,
    pub name: String,
    pub old_name: Option<String>,
    pub old_path: Option<String>,
    pub old_path_display: String,
    /// Android-style operation label (e.g. "Created file", "Renamed folder").
    pub operation_label: String,
    pub author_email: String,
    pub author_name: String,
    /// Relative avatar URL; empty once the author is deleted.
    pub author_avatar_url: String,
    /// Raw Unix seconds; also the local-day boundary the group headers cut on.
    pub time_ts: i64,
    /// Number of items in a batch operation (1 for single operations).
    pub batch_count: usize,
    pub detail_items: Vec<String>,
    pub old_repo_name: Option<String>,
}

impl ActivityView {
    pub fn has_old_path(&self) -> bool {
        self.old_path.is_some()
    }
}

/// Android-style operation label matching seadroid's `SystemSwitchUtils.obj_type`.
fn operation_label(t: &I18n, op_type: &str, obj_type: &str, count: usize) -> String {
    let plural = count > 1;
    match (op_type, obj_type) {
        ("create", "repo") => t.tr("activity.created_library").to_string(),
        ("create", "dir") => {
            if plural {
                t.trf("activity.created_some_folders", &[("n", count.to_string())])
            } else {
                t.tr("activity.created_new_folder").to_string()
            }
        }
        ("create", "file") => {
            if plural {
                t.trf("activity.created_some_files", &[("n", count.to_string())])
            } else {
                t.tr("activity.created_new_file").to_string()
            }
        }
        ("batch_create", "dir") => {
            t.trf("activity.created_some_folders", &[("n", count.to_string())])
        }
        ("batch_create", "file") => {
            t.trf("activity.created_some_files", &[("n", count.to_string())])
        }
        ("delete", "repo") => t.tr("activity.deleted_library").to_string(),
        ("delete", "dir") => {
            if plural {
                t.trf("activity.deleted_some_folders", &[("n", count.to_string())])
            } else {
                t.tr("activity.deleted_folder").to_string()
            }
        }
        ("delete", "file") => {
            if plural {
                t.trf("activity.deleted_some_files", &[("n", count.to_string())])
            } else {
                t.tr("activity.deleted_file").to_string()
            }
        }
        ("batch_delete", "dir") => {
            t.trf("activity.deleted_some_folders", &[("n", count.to_string())])
        }
        ("batch_delete", "file") => {
            t.trf("activity.deleted_some_files", &[("n", count.to_string())])
        }
        ("edit", "repo") => t.tr("activity.edited_library").to_string(),
        ("edit", "dir") => t.tr("activity.edited_folder").to_string(),
        ("edit", "file") => t.tr("activity.edited_file").to_string(),
        ("rename", "repo") => t.tr("activity.renamed_library").to_string(),
        ("rename", "dir") => t.tr("activity.renamed_folder").to_string(),
        ("rename", "file") => t.tr("activity.renamed_file").to_string(),
        ("move", "dir") => t.tr("activity.moved_folder").to_string(),
        ("move", "file") => t.tr("activity.moved_file").to_string(),
        ("recover", "repo") => t.tr("activity.recovered_library").to_string(),
        ("recover", "dir") => t.tr("activity.recovered_folder").to_string(),
        ("recover", "file") => t.tr("activity.recovered_file").to_string(),
        ("clean-up-trash", _) => t.tr("activity.trash_cleaned").to_string(),
        _ => t.tr("activity.operation").to_string(),
    }
}

/// GET /activities/ — list file activity history.
pub async fn activities_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let events = state
        .repos
        .activity
        .find_recent_by_user(user.user_id, 50)
        .await?;

    let t = I18n::get(user.language.as_deref());

    let mut repo_cache: HashMap<String, Option<String>> = HashMap::new();
    for e in &events {
        #[allow(clippy::map_entry)]
        if !repo_cache.contains_key(&e.repo_id) {
            let r = state.repos.repo.find_by_id(&e.repo_id).await?;
            repo_cache.insert(e.repo_id.clone(), r.map(|r| r.name));
        }
    }

    let mut user_cache: HashMap<i32, Option<(String, String)>> = HashMap::new();
    for e in &events {
        #[allow(clippy::map_entry)]
        if !user_cache.contains_key(&e.user_id) {
            let u = state.repos.user.find_by_id(e.user_id).await?;
            user_cache.insert(e.user_id, u.map(|u| (u.nickname(), u.email)));
        }
    }

    let mut activities = Vec::with_capacity(events.len());

    for e in &events {
        let repo_name = repo_cache
            .get(&e.repo_id)
            .cloned()
            .flatten()
            .unwrap_or_default();

        let (author_name, email) = user_cache
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

        let author_avatar_url = crate::service::user::primary_avatar_url(
            if email.is_empty() { "deleted" } else { &email },
            32,
        );

        let old_path_display = e.old_path.as_deref().unwrap_or("").to_string();

        // Detail JSON: batch item names, repo rename info, and the server-side
        // merge markers (see `ActivityItem`); a merged row must not read as a
        // device saving this path.
        let (batch_count, detail_items, old_repo_name, merged, merge_kept) =
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
                    let flag = |key: &str| {
                        arr.iter()
                            .any(|d| d.get(key).and_then(|v| v.as_bool()).unwrap_or(false))
                    };
                    (count, items, orn, flag("merge"), flag("merge_kept"))
                }
                Ok(serde_json::Value::Object(obj)) => {
                    let orn = obj
                        .get("old_repo_name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let flag = |key: &str| obj.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
                    (1, vec![], orn, flag("merge"), flag("merge_kept"))
                }
                _ => (1, vec![], None, false, false),
            };

        let old_name = e
            .old_path
            .as_deref()
            .and_then(|p| p.rsplit_once('/').map(|(_, n)| n.to_string()));

        let operation_label = operation_label(t, &e.op_type, &e.obj_type, batch_count);
        // Say who produced the row when it was not a device saving this path.
        let operation_label = if merge_kept {
            format!("{} · {}", operation_label, t.tr("activity.merge_kept_path"))
        } else if merged {
            format!(
                "{} · {}",
                operation_label,
                t.tr("activity.merged_by_server")
            )
        } else {
            operation_label
        };

        activities.push(ActivityView {
            op_type: e.op_type.clone(),
            obj_type: e.obj_type.clone(),
            repo_id: e.repo_id.clone(),
            repo_name,
            path: e.path.clone(),
            name,
            old_name,
            old_path: e.old_path.clone(),
            old_path_display,
            operation_label,
            author_email: email,
            author_name,
            author_avatar_url,
            time_ts: e.created_at,
            batch_count,
            detail_items,
            old_repo_name,
        });
    }

    let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;
    let tpl = ActivitiesTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        activities,
        active_page: "activities",
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}
