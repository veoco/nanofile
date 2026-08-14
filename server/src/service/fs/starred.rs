use std::collections::HashMap;
use std::sync::Arc;

use crate::fs::core::{
    read_fs_dir_data, read_fs_dir_data_batch, resolve_fs_id, resolve_fs_ids_batch,
};
use crate::repository::Repositories;
use base::error::AppError;
use chrono::{DateTime, Utc};
use infra::common::util::basename;
use infra::serialization::S_IFDIR;

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
}

impl StarredService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
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
        let user_nickname = self
            .repos
            .user
            .find_by_id(user_id)
            .await?
            .map(|u| u.nickname())
            .unwrap_or_else(|| email.split('@').next().unwrap_or("").to_string());

        let entries = self.repos.starred.find_by_user_id(user_id).await?;

        let mut repo_cache: HashMap<String, Option<infra::entity::repo::Model>> = HashMap::new();
        for entry in &entries {
            if !repo_cache.contains_key(&entry.repo_id) {
                let r = self.repos.repo.find_by_id(&entry.repo_id).await?;
                repo_cache.insert(entry.repo_id.clone(), r);
            }
        }

        // Cache the head commit per repo so mtime resolution does not re-fetch
        // the head commit for every starred item in the same repo.
        let mut head_cache: HashMap<String, Option<infra::entity::commit::Model>> = HashMap::new();
        for entry in &entries {
            if !head_cache.contains_key(&entry.repo_id) {
                let head = match repo_cache.get(&entry.repo_id).and_then(|o| o.as_ref()) {
                    Some(repo) => match &repo.head_commit_id {
                        Some(cid) => self.repos.commit.find_by_id(cid).await?,
                        None => None,
                    },
                    None => None,
                };
                head_cache.insert(entry.repo_id.clone(), head);
            }
        }

        let mut starred_repos = Vec::new();
        let mut starred_folders = Vec::new();
        let mut starred_files = Vec::new();

        let mtime_map =
            batch_resolve_mtime_deleted(&self.repos, &entries, &repo_cache, &head_cache).await;

        for entry in &entries {
            let repo_opt = repo_cache.get(&entry.repo_id).and_then(|o| o.as_ref());
            let (mtime, deleted) = if entry.path == "/" {
                let m = repo_opt.map(|r| r.updated_at).unwrap_or(0);
                (m, repo_opt.is_none())
            } else {
                *mtime_map
                    .get(&(entry.repo_id.clone(), entry.path.clone()))
                    .unwrap_or(&(0, true))
            };
            let item = build_item_json(entry, repo_opt, email, &user_nickname, mtime, deleted);

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
            let name = basename(clean_path);

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

            let parent_fs_id = resolve_fs_id(&self.repos, repo_id, &head.root_id, parent_path)
                .await
                .map_err(|_| AppError::NotFound(format!("Item {path} not found.")))?;

            let parent_data = read_fs_dir_data(&self.repos, repo_id, &parent_fs_id)
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

        // Resolve the head commit once for mtime resolution (no-op for repo stars).
        let head_for_json = if normalized_path == "/" {
            None
        } else {
            match repo_record.head_commit_id.as_ref() {
                Some(cid) => self.repos.commit.find_by_id(cid).await?,
                None => None,
            }
        };

        // Check for duplicate
        let existing = self
            .repos
            .starred
            .find_by_user_repo_and_path(user_id, repo_id, &normalized_path)
            .await?;

        if let Some(ref entry) = existing {
            let (mtime, deleted) = if normalized_path == "/" {
                (repo_record.updated_at, false)
            } else {
                get_entry_mtime_or_deleted(&self.repos, entry, head_for_json.as_ref()).await
            };
            return Ok(build_item_json(
                entry,
                Some(&repo_record),
                email,
                &user_nickname,
                mtime,
                deleted,
            ));
        }

        // Insert
        let now = Utc::now().timestamp();
        self.repos
            .starred
            .create_starred(crate::repository::starred::CreateStarredParams {
                repo_id: repo_id.to_string(),
                path: normalized_path.clone(),
                user_id,
                is_dir,
                created_at: now,
            })
            .await?;

        let new_entry = self
            .repos
            .starred
            .find_by_user_repo_and_path(user_id, repo_id, &normalized_path)
            .await?
            .ok_or_else(|| {
                AppError::Internal("failed to find starred entry after insert".into())
            })?;

        let (mtime, deleted) = if normalized_path == "/" {
            (repo_record.updated_at, false)
        } else {
            get_entry_mtime_or_deleted(&self.repos, &new_entry, head_for_json.as_ref()).await
        };
        Ok(build_item_json(
            &new_entry,
            Some(&repo_record),
            email,
            &user_nickname,
            mtime,
            deleted,
        ))
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

fn build_item_json(
    entry: &infra::entity::starred_file::Model,
    repo_opt: Option<&infra::entity::repo::Model>,
    auth_email: &str,
    user_nickname: &str,
    mtime: i64,
    deleted: bool,
) -> serde_json::Value {
    let (repo_name, repo_encrypted) = match repo_opt {
        Some(r) => (r.name.clone(), r.encrypted != 0),
        None => (String::new(), false),
    };

    let obj_name = if entry.path == "/" {
        repo_name.clone()
    } else {
        entry
            .path
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|(_, n)| n.to_string())
            .unwrap_or_default()
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

/// Batch-resolve `(mtime, deleted)` for all non-"/" starred entries, grouping
/// by repo and resolving every parent path in a shared level-frontier walk
/// instead of one `resolve_fs_id` + `read_fs_dir_data` round-trip per entry.
async fn batch_resolve_mtime_deleted(
    repos: &Repositories,
    entries: &[infra::entity::starred_file::Model],
    repo_cache: &HashMap<String, Option<infra::entity::repo::Model>>,
    head_cache: &HashMap<String, Option<infra::entity::commit::Model>>,
) -> HashMap<(String, String), (i64, bool)> {
    let mut out: HashMap<(String, String), (i64, bool)> = HashMap::new();

    struct Item {
        key: (String, String),
        root_id: String,
        parent_path: String,
        name: String,
    }
    let mut by_repo: HashMap<String, Vec<Item>> = HashMap::new();

    for entry in entries {
        let key = (entry.repo_id.clone(), entry.path.clone());
        if entry.path == "/" {
            continue;
        }
        let repo_opt = repo_cache.get(&entry.repo_id).and_then(|o| o.as_ref());
        let head_opt = head_cache.get(&entry.repo_id).and_then(|o| o.as_ref());
        let (Some(_repo), Some(head)) = (repo_opt, head_opt) else {
            out.insert(key, (0, true));
            continue;
        };
        let path = entry.path.trim_end_matches('/');
        let (parent_path, name) = match path.rsplit_once('/') {
            Some(("", n)) => ("/", n),
            Some((p, n)) => (p, n),
            None => {
                out.insert(key, (0, true));
                continue;
            }
        };
        by_repo
            .entry(entry.repo_id.clone())
            .or_default()
            .push(Item {
                key,
                root_id: head.root_id.clone(),
                parent_path: parent_path.to_string(),
                name: name.to_string(),
            });
    }

    for (repo_id, items) in by_repo {
        let targets: Vec<(String, String)> = items
            .iter()
            .map(|it| (it.root_id.clone(), it.parent_path.clone()))
            .collect();
        let resolved = match resolve_fs_ids_batch(repos, &repo_id, &targets).await {
            Ok(r) => r,
            Err(_) => {
                for it in &items {
                    out.insert(it.key.clone(), (0, true));
                }
                continue;
            }
        };

        let mut to_read: Vec<(String, usize)> = Vec::new();
        for (i, it) in items.iter().enumerate() {
            match &resolved[i] {
                Some(fsid) => to_read.push((fsid.clone(), i)),
                None => {
                    out.insert(it.key.clone(), (0, true));
                }
            }
        }

        let fs_ids: Vec<String> = to_read.iter().map(|(id, _)| id.clone()).collect();
        let dir_map = match read_fs_dir_data_batch(repos, &repo_id, &fs_ids).await {
            Ok(m) => m,
            Err(_) => {
                for (_, i) in &to_read {
                    out.insert(items[*i].key.clone(), (0, true));
                }
                continue;
            }
        };

        for (parent_fs_id, i) in to_read {
            let it = &items[i];
            match dir_map
                .get(&parent_fs_id)
                .and_then(|d| d.dirents.iter().find(|e| e.name == it.name))
            {
                Some(dirent) => {
                    out.insert(it.key.clone(), (dirent.mtime, false));
                }
                None => {
                    out.insert(it.key.clone(), (0, true));
                }
            }
        }
    }

    out
}

async fn get_entry_mtime_or_deleted(
    repos: &Repositories,
    entry: &infra::entity::starred_file::Model,
    head: Option<&infra::entity::commit::Model>,
) -> (i64, bool) {
    let Some(head) = head else {
        return (0, true);
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

    let parent_fs_id = match resolve_fs_id(repos, &entry.repo_id, &head.root_id, parent_path).await
    {
        Ok(id) => id,
        Err(_) => return (0, true),
    };

    let parent_data = match read_fs_dir_data(repos, &entry.repo_id, &parent_fs_id).await {
        Ok(d) => d,
        Err(_) => return (0, true),
    };

    match parent_data.dirents.iter().find(|d| d.name == name) {
        Some(dirent) => (dirent.mtime, false),
        None => (0, true),
    }
}
