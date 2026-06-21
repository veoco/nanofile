/// Web UI repo handlers — list, create, delete repos.
use askama::Template;
use axum::{extract::State, response::Html};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::activity_log;
use crate::entity::{repo, repo_member, sync_token};
use crate::error::AppError;
use crate::ui::files::format_size;

use super::auth_extractor::WebUser;

// ─── Templates ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "repos/list.html")]
pub struct RepoListTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub repos: Vec<RepoInfo>,
    pub active_page: &'static str,
    pub user_id: i32,
    pub csrf_token: String,
}

// ─── Data types ──────────────────────────────────────────────────────────────

pub struct RepoInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub size: i64,
    pub size_display: String,
    pub mtime: i64,
    pub encrypted: bool,
    pub owner_id: i32,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /libraries/ — list user's repos.
pub async fn list_repos(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let db = state.db.as_ref();

    // Find repos where user is a member
    let memberships = repo_member::Entity::find()
        .filter(repo_member::Column::UserId.eq(user.user_id))
        .all(db)
        .await
        .map_err(|e| AppError::internal(format!("db error: {e}")))?;

    let mut repos = Vec::new();
    for membership in memberships {
        if let Some(r) = repo::Entity::find_by_id(membership.repo_id)
            .one(db)
            .await
            .map_err(|e| AppError::internal(format!("db error: {e}")))?
        {
            repos.push(RepoInfo {
                id: r.id,
                name: r.name,
                description: r.description,
                size: r.size,
                size_display: format_size(r.size),
                mtime: r.updated_at,
                encrypted: r.encrypted != 0,
                owner_id: r.owner_id,
            });
        }
    }

    let csrf_token =
        crate::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);

    let tpl = RepoListTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        is_admin: user.is_admin,
        repos,
        active_page: "repos",
        user_id: user.user_id,
        csrf_token,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /libraries/create/ — create a new library.
pub async fn create_repo(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;

    let name = form.get("name").map(|s| s.trim()).unwrap_or("");
    if name.is_empty() || name.contains('/') {
        return Err(AppError::BadRequest("invalid repo name".into()));
    }

    let db = state.db.as_ref();
    let repo_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    let model = repo::ActiveModel {
        id: Set(repo_id.clone()),
        name: Set(name.to_string()),
        description: Set(String::new()),
        owner_id: Set(user.user_id),
        encrypted: Set(0i8),
        enc_version: Set(0i8),
        magic: Set(None),
        random_key: Set(None),
        salt: Set(String::new()),
        head_commit_id: Set(None),
        permission: Set("rw".to_string()),
        repo_version: Set(1),
        size: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
    };
    repo::Entity::insert(model).exec(db).await?;

    repo_member::Entity::insert(repo_member::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_id.clone()),
        user_id: Set(user.user_id),
        permission: Set("rw".to_string()),
        created_at: Set(now),
    })
    .exec(db)
    .await?;

    // Log repo creation activity
    activity_log::log_activity(
        db,
        &repo_id,
        "create",
        "repo",
        "/",
        user.user_id,
        None,
        None,
        None,
        None,
        None,
    )
    .await;

    Ok(axum::response::Redirect::to("/libraries/"))
}

/// POST /library/{id}/rename — rename a library.
pub async fn rename_repo(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(repo_id): axum::extract::Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;

    let db = state.db.as_ref();

    let r = repo::Entity::find_by_id(&repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    if r.owner_id != user.user_id {
        return Err(AppError::Forbidden);
    }

    let new_name = form.get("name").map(|s| s.trim()).unwrap_or("");
    if new_name.is_empty() || new_name.contains('/') {
        return Err(AppError::BadRequest("invalid repo name".into()));
    }

    // Log repo rename activity (before update, so detail captures the old name).
    activity_log::log_activity(
        db,
        &repo_id,
        "rename",
        "repo",
        "/",
        user.user_id,
        None,
        None,
        None,
        Some(&r.name),
        None,
    )
    .await;

    let now = chrono::Utc::now().timestamp();
    let mut active: repo::ActiveModel = r.into();
    active.name = Set(new_name.to_string());
    active.updated_at = Set(now);
    active.update(db).await?;

    Ok(axum::response::Redirect::to("/libraries/"))
}

/// POST /library/{id}/delete — delete a library.
pub async fn delete_repo(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(repo_id): axum::extract::Path<String>,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    crate::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;
    let db = state.db.as_ref();

    let r = repo::Entity::find_by_id(&repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    if r.owner_id != user.user_id {
        return Err(AppError::Forbidden);
    }

    // --- REPO TRASH: Record deleted repo before cascade-delete ---
    if let Err(e) = crate::repo::trash::TrashService::add_deleted_repo(
        db,
        &repo_id,
        &r.name,
        r.head_commit_id.as_deref(),
        r.owner_id,
        r.size,
    )
    .await
    {
        tracing::warn!("Failed to record deleted repo in trash: {e}");
    }
    // --- END REPO TRASH ---

    // Log repo deletion activity BEFORE deleting related records (FK constraint
    // prevents inserting activity with a non-existent repo_id).
    activity_log::log_activity(
        db,
        &repo_id,
        "delete",
        "repo",
        "/",
        user.user_id,
        None,
        None,
        None,
        None,
        None,
    )
    .await;

    repo_member::Entity::delete_many()
        .filter(repo_member::Column::RepoId.eq(&repo_id))
        .exec(db)
        .await?;

    sync_token::Entity::delete_many()
        .filter(sync_token::Column::RepoId.eq(&repo_id))
        .exec(db)
        .await?;

    repo::Entity::delete_by_id(&repo_id).exec(db).await?;

    Ok(axum::response::Redirect::to("/libraries/"))
}
