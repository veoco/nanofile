use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::api::repos::extract_multipart_field;
use crate::auth::middleware::AuthUser;
use crate::entity::{commit, repo, starred_file};
use crate::error::AppError;
use crate::serialization::S_IFDIR;
use crate::storage::read_fs_dir_data;
use crate::storage::resolve_fs_id;

#[derive(Deserialize)]
pub struct StarOrUnstarRequest {
    pub repo_id: String,
    pub path: String,
}

#[derive(Deserialize)]
pub struct UnstarQuery {
    pub repo_id: String,
    pub path: String,
}

// ── Helpers ───────────────────────────────────────────────────────────

fn timestamp_to_iso(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

fn email_local_part(email: &str) -> &str {
    email.split('@').next().unwrap_or("")
}

// ── GET /api/v2.1/starred-items/ ──────────────────────────────────────

pub async fn get_starred_items(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db = state.db.as_ref();
    let entries = starred_file::Entity::find()
        .filter(starred_file::Column::UserId.eq(auth.user_id))
        .all(db)
        .await?;

    // Build a cache of repo lookups to avoid N+1 queries on repos
    let mut repo_cache: std::collections::HashMap<String, Option<repo::Model>> =
        std::collections::HashMap::new();
    for entry in &entries {
        if !repo_cache.contains_key(&entry.repo_id) {
            let r = repo::Entity::find_by_id(&entry.repo_id).one(db).await?;
            repo_cache.insert(entry.repo_id.clone(), r);
        }
    }

    let mut starred_repos = Vec::new();
    let mut starred_folders = Vec::new();
    let mut starred_files = Vec::new();

    for entry in &entries {
        let repo_opt = repo_cache.get(&entry.repo_id).and_then(|o| o.as_ref());
        let item = build_item_json(db, entry, repo_opt, &auth.email).await;

        if entry.path == "/" {
            starred_repos.push(item);
        } else if entry.is_dir {
            starred_folders.push(item);
        } else {
            starred_files.push(item);
        }
    }

    // Sort by mtime descending
    let sort_by_mtime_desc = |a: &serde_json::Value, b: &serde_json::Value| {
        let am = a["mtime"].as_str().unwrap_or("");
        let bm = b["mtime"].as_str().unwrap_or("");
        bm.cmp(am)
    };
    starred_repos.sort_by(sort_by_mtime_desc);
    starred_folders.sort_by(sort_by_mtime_desc);
    starred_files.sort_by(sort_by_mtime_desc);

    let all_items: Vec<serde_json::Value> = starred_repos
        .into_iter()
        .chain(starred_folders)
        .chain(starred_files)
        .collect();

    Ok(Json(json!({"starred_item_list": all_items})))
}

// ── POST /api/v2.1/starred-items/ ─────────────────────────────────────

pub async fn star_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    let db = state.db.as_ref();

    // Parse request (JSON or multipart)
    let req = if headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("json"))
    {
        serde_json::from_slice::<StarOrUnstarRequest>(&bytes)?
    } else {
        StarOrUnstarRequest {
            repo_id: extract_multipart_field(&bytes, "repo_id")
                .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?,
            path: extract_multipart_field(&bytes, "path")
                .ok_or_else(|| AppError::BadRequest("path required".into()))?,
        }
    };

    if req.repo_id.is_empty() {
        return Err(AppError::BadRequest("repo_id invalid.".into()));
    }
    if req.path.is_empty() {
        return Err(AppError::BadRequest("path invalid.".into()));
    }

    // 1. Validate repo exists
    let repo_record = repo::Entity::find_by_id(&req.repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("Library {} not found.", req.repo_id)))?;

    // 2. Validate path exists and determine is_dir
    let (normalized_path, is_dir) = if req.path == "/" || req.path.is_empty() {
        ("/".to_string(), true)
    } else {
        let path = req.path.trim_end_matches('/');
        let parent_path = match path.rsplit_once('/') {
            Some(("", _)) => "/",
            Some((parent, _)) => parent,
            None => "/",
        };
        let name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

        let head_cid = repo_record
            .head_commit_id
            .as_ref()
            .ok_or_else(|| AppError::NotFound("No commits in library.".into()))?;
        let head = commit::Entity::find()
            .filter(commit::Column::CommitId.eq(head_cid))
            .one(db)
            .await?
            .ok_or_else(|| AppError::NotFound("Head commit not found.".into()))?;

        let parent_fs_id = resolve_fs_id(
            db,
            &req.repo_id,
            &head.root_id,
            parent_path,
            Some(state.path_cache.as_ref()),
        )
        .await
        .map_err(|_| AppError::NotFound(format!("Item {} not found.", req.path)))?;

        let parent_data = read_fs_dir_data(db, &req.repo_id, &parent_fs_id)
            .await
            .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;

        let dirent = parent_data
            .dirents
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| AppError::NotFound(format!("Item {} not found.", req.path)))?;

        let is_dir_flag = dirent.mode & S_IFDIR != 0;
        let normalized = if is_dir_flag {
            format!("{}/", path)
        } else {
            path.to_string()
        };

        (normalized, is_dir_flag)
    };

    // 3. Permission check (read access is sufficient for starring)
    crate::storage::check_repo_read_permission(db, &req.repo_id, auth.user_id).await?;

    // 4. Check for duplicate
    let existing = starred_file::Entity::find()
        .filter(starred_file::Column::UserId.eq(auth.user_id))
        .filter(starred_file::Column::RepoId.eq(&req.repo_id))
        .filter(starred_file::Column::Path.eq(&normalized_path))
        .one(db)
        .await?;

    if let Some(ref entry) = existing {
        let item = build_item_json(db, entry, Some(&repo_record), &auth.email).await;
        return Ok(Json(item));
    }

    // 5. Insert
    let now = Utc::now().timestamp();
    let new_entry = starred_file::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(req.repo_id),
        path: Set(normalized_path),
        user_id: Set(auth.user_id),
        is_dir: Set(is_dir),
        created_at: Set(now),
    }
    .insert(db)
    .await?;

    let item = build_item_json(db, &new_entry, Some(&repo_record), &auth.email).await;
    Ok(Json(item))
}

// ── DELETE /api/v2.1/starred-items/ ───────────────────────────────────

pub async fn unstar_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<UnstarQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db = state.db.as_ref();

    // Check existence
    let existing = starred_file::Entity::find()
        .filter(starred_file::Column::UserId.eq(auth.user_id))
        .filter(starred_file::Column::RepoId.eq(&query.repo_id))
        .filter(starred_file::Column::Path.eq(&query.path))
        .one(db)
        .await?;

    if existing.is_none() {
        return Err(AppError::NotFound(format!(
            "Item {} not found.",
            query.path
        )));
    }

    starred_file::Entity::delete_many()
        .filter(starred_file::Column::UserId.eq(auth.user_id))
        .filter(starred_file::Column::RepoId.eq(&query.repo_id))
        .filter(starred_file::Column::Path.eq(&query.path))
        .exec(db)
        .await?;

    Ok(Json(json!({"success": true})))
}

// ── Item JSON builder (shared by GET and POST) ────────────────────────

async fn build_item_json(
    db: &sea_orm::DatabaseConnection,
    entry: &starred_file::Model,
    repo_opt: Option<&repo::Model>,
    auth_email: &str,
) -> serde_json::Value {
    let (repo_name, repo_encrypted) = match repo_opt {
        Some(r) => (r.name.clone(), r.encrypted != 0),
        None => (String::new(), false),
    };

    let (obj_name, mtime, deleted) = if entry.path == "/" {
        let m = repo_opt.map(|r| r.updated_at).unwrap_or(0);
        (repo_name.clone(), m, repo_opt.is_none())
    } else {
        let name = entry
            .path
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|(_, n)| n.to_string())
            .unwrap_or_default();
        let (m, d) = if let Some(repo) = repo_opt {
            get_entry_mtime_or_deleted(db, repo, entry).await
        } else {
            (0, true)
        };
        (name, m, d)
    };

    json!({
        "repo_id": entry.repo_id,
        "repo_name": repo_name,
        "repo_encrypted": repo_encrypted,
        "is_dir": entry.is_dir,
        "path": entry.path,
        "obj_name": obj_name,
        "mtime": timestamp_to_iso(mtime),
        "deleted": deleted,
        "user_email": auth_email,
        "user_name": email_local_part(auth_email),
        "user_contact_email": auth_email,
    })
}

/// Look up a dirent's mtime by walking the FS tree.
/// Returns `(mtime, deleted)` — where `deleted` is true if the entry is gone.
async fn get_entry_mtime_or_deleted(
    db: &sea_orm::DatabaseConnection,
    repo: &repo::Model,
    entry: &starred_file::Model,
) -> (i64, bool) {
    let head_cid = match repo.head_commit_id.as_ref() {
        Some(c) => c.clone(),
        None => return (0, true),
    };

    let head = match commit::Entity::find()
        .filter(commit::Column::CommitId.eq(&head_cid))
        .one(db)
        .await
    {
        Ok(Some(h)) => h,
        _ => return (0, true),
    };

    let path = entry.path.trim_end_matches('/');
    let parent_path = match path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((p, _)) => p,
        None => "/",
    };
    let name = match path.rsplit_once('/') {
        Some((_, n)) => n,
        None => return (0, true),
    };

    let parent_fs_id =
        match resolve_fs_id(db, &entry.repo_id, &head.root_id, parent_path, None).await {
            Ok(id) => id,
            Err(_) => return (0, true),
        };

    let parent_data = match read_fs_dir_data(db, &entry.repo_id, &parent_fs_id).await {
        Ok(d) => d,
        Err(_) => return (0, true),
    };

    match parent_data.dirents.iter().find(|d| d.name == name) {
        Some(dirent) => (dirent.mtime, false),
        None => (0, true),
    }
}
