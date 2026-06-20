use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::error::AppError;
use crate::repo::{read_fs_dir_data, resolve_fs_id};
use crate::repository::Repositories;
use crate::serialization::S_IFDIR;

#[derive(serde::Serialize)]
pub struct StarredFileEntry {
    pub repo_id: String,
    pub path: String,
    pub size: Option<i64>,
    pub last_modified: Option<i64>,
    pub is_dir: bool,
}

pub struct StarredService {
    repos: Arc<Repositories>,
    db: Arc<DatabaseConnection>,
}

impl StarredService {
    pub fn new(repos: Arc<Repositories>, db: Arc<DatabaseConnection>) -> Self {
        Self { repos, db }
    }

    fn db(&self) -> &DatabaseConnection {
        self.db.as_ref()
    }

    /// List starred files (legacy v2 API).
    pub async fn list_starred_files(
        &self,
        user_id: i32,
    ) -> Result<Vec<StarredFileEntry>, AppError> {
        let entries = self.repos.starred.find_by_user_id(user_id).await?;

        Ok(entries
            .into_iter()
            .map(|e| StarredFileEntry {
                repo_id: e.repo_id,
                path: e.path,
                size: None,
                last_modified: None,
                is_dir: e.is_dir,
            })
            .collect())
    }

    /// Get starred items (v2.1 API).
    pub async fn get_starred_items(
        &self,
        user_id: i32,
        email: &str,
    ) -> Result<serde_json::Value, AppError> {
        let db = self.db();

        let user_nickname = self
            .repos
            .user
            .find_by_id(user_id)
            .await?
            .map(|u| u.nickname())
            .unwrap_or_else(|| email.split('@').next().unwrap_or("").to_string());

        let entries = self.repos.starred.find_by_user_id(user_id).await?;

        let mut repo_cache: HashMap<String, Option<crate::entity::repo::Model>> = HashMap::new();
        for entry in &entries {
            if !repo_cache.contains_key(&entry.repo_id) {
                let r = self.repos.repo.find_by_id(&entry.repo_id).await?;
                repo_cache.insert(entry.repo_id.clone(), r);
            }
        }

        let mut starred_repos = Vec::new();
        let mut starred_folders = Vec::new();
        let mut starred_files = Vec::new();

        for entry in &entries {
            let repo_opt = repo_cache.get(&entry.repo_id).and_then(|o| o.as_ref());
            let item = build_item_json(db, entry, repo_opt, email, &user_nickname).await;

            if entry.path == "/" {
                starred_repos.push(item);
            } else if entry.is_dir {
                starred_folders.push(item);
            } else {
                starred_files.push(item);
            }
        }

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

        Ok(serde_json::json!({"starred_item_list": all_items}))
    }

    /// Star an item (v2.1 API).
    pub async fn star_item(
        &self,
        user_id: i32,
        email: &str,
        repo_id: &str,
        path: &str,
    ) -> Result<serde_json::Value, AppError> {
        let db = self.db();

        if repo_id.is_empty() {
            return Err(AppError::BadRequest("repo_id invalid.".into()));
        }
        if path.is_empty() {
            return Err(AppError::BadRequest("path invalid.".into()));
        }

        let repo_record = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("Library {repo_id} not found.")))?;

        let (normalized_path, is_dir) = if path == "/" || path.is_empty() {
            ("/".to_string(), true)
        } else {
            let clean_path = path.trim_end_matches('/');
            let parent_path = match clean_path.rsplit_once('/') {
                Some(("", _)) => "/",
                Some((parent, _)) => parent,
                None => "/",
            };
            let name = clean_path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

            let head_cid = repo_record
                .head_commit_id
                .as_ref()
                .ok_or_else(|| AppError::NotFound("No commits in library.".into()))?;
            let head = self
                .repos
                .commit
                .find_by_id(head_cid)
                .await?
                .ok_or_else(|| AppError::NotFound("Head commit not found.".into()))?;

            let parent_fs_id = resolve_fs_id(db, repo_id, &head.root_id, parent_path)
                .await
                .map_err(|_| AppError::NotFound(format!("Item {path} not found.")))?;

            let parent_data = read_fs_dir_data(db, repo_id, &parent_fs_id)
                .await
                .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;

            let dirent = parent_data
                .dirents
                .iter()
                .find(|d| d.name == name)
                .ok_or_else(|| AppError::NotFound(format!("Item {path} not found.")))?;

            let is_dir_flag = dirent.mode & S_IFDIR != 0;
            (clean_path.to_string(), is_dir_flag)
        };

        let user_nickname = self
            .repos
            .user
            .find_by_id(user_id)
            .await?
            .map(|u| u.nickname())
            .unwrap_or_else(|| email.split('@').next().unwrap_or("").to_string());

        // Check for duplicate
        let existing = self
            .repos
            .starred
            .find_by_user_repo_and_path(user_id, repo_id, &normalized_path)
            .await?;

        if let Some(ref entry) = existing {
            return Ok(build_item_json(db, entry, Some(&repo_record), email, &user_nickname).await);
        }

        // Insert
        let now = Utc::now().timestamp();
        let new_entry = crate::entity::starred_file::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            path: Set(normalized_path),
            user_id: Set(user_id),
            is_dir: Set(is_dir),
            created_at: Set(now),
        }
        .insert(db)
        .await?;

        Ok(build_item_json(db, &new_entry, Some(&repo_record), email, &user_nickname).await)
    }

    /// Unstar an item.
    pub async fn unstar_item(
        &self,
        user_id: i32,
        repo_id: &str,
        path: &str,
    ) -> Result<(), AppError> {
        let existing = self
            .repos
            .starred
            .find_by_user_repo_and_path(user_id, repo_id, path)
            .await?;

        if existing.is_none() {
            return Err(AppError::NotFound(format!("Item {path} not found.")));
        }

        self.repos
            .starred
            .delete_by_user_repo_and_path(user_id, repo_id, path)
            .await?;

        Ok(())
    }
}

fn timestamp_to_iso(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

async fn build_item_json(
    db: &DatabaseConnection,
    entry: &crate::entity::starred_file::Model,
    repo_opt: Option<&crate::entity::repo::Model>,
    auth_email: &str,
    user_nickname: &str,
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

    serde_json::json!({
        "repo_id": entry.repo_id,
        "repo_name": repo_name,
        "repo_encrypted": repo_encrypted,
        "is_dir": entry.is_dir,
        "path": entry.path,
        "obj_name": obj_name,
        "mtime": timestamp_to_iso(mtime),
        "deleted": deleted,
        "user_email": auth_email,
        "user_name": user_nickname,
        "user_contact_email": auth_email,
    })
}

async fn get_entry_mtime_or_deleted(
    db: &DatabaseConnection,
    repo: &crate::entity::repo::Model,
    entry: &crate::entity::starred_file::Model,
) -> (i64, bool) {
    let head_cid = match repo.head_commit_id.as_ref() {
        Some(c) => c.clone(),
        None => return (0, true),
    };

    let head = match crate::entity::commit::Entity::find()
        .filter(crate::entity::commit::Column::CommitId.eq(&head_cid))
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

    let parent_fs_id = match resolve_fs_id(db, &entry.repo_id, &head.root_id, parent_path).await {
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
