use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::repository::Repositories;
use crate::service::auth::token::generate_sync_token;
use base::common::SEAF_METADATA_TYPE_DIR;
use base::error::AppError;
use infra::serialization::pack_fs;

/// Service for sync protocol operations.
pub struct SyncService {
    repos: Arc<Repositories>,
}

impl SyncService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// Get all repos accessible to a user with their sync tokens.
    pub async fn accessible_repos(&self, user_id: i32) -> Result<Vec<AccessibleRepo>, AppError> {
        let memberships = self.repos.member.find_by_user_id(user_id).await?;
        let mut result = Vec::new();
        for member in &memberships {
            let r = self.repos.repo.find_by_id(&member.repo_id).await?;
            if let Some(r) = r {
                let token = self.get_or_create_sync_token(&r.id, user_id).await?;
                let owner = self.repos.user.find_by_id(r.owner_id).await?;
                result.push(AccessibleRepo {
                    repo_id: r.id.clone(),
                    repo_name: r.name,
                    repo_desc: r.description,
                    owner_email: owner.map(|u| u.email).unwrap_or_default(),
                    token,
                    permission: member.permission.clone(),
                });
            }
        }
        Ok(result)
    }

    /// Validate a sync token for a given repo and return user permissions.
    pub async fn folder_perm_for_repo(
        &self,
        repo_id: &str,
        token: &str,
    ) -> Result<FolderPermResult, AppError> {
        let token_valid = self
            .repos
            .sync_token
            .find_by_token_and_repo(token, repo_id)
            .await?
            .is_some();

        if token_valid {
            let memberships = self.repos.member.find_by_repo_id(repo_id).await?;
            let permission = memberships
                .first()
                .map(|m| m.permission.clone())
                .unwrap_or_else(|| "rw".to_string());
            Ok(FolderPermResult {
                valid: true,
                permission,
            })
        } else {
            Ok(FolderPermResult {
                valid: false,
                permission: String::new(),
            })
        }
    }

    /// Get head commits for a list of repo IDs.
    pub async fn head_commits_multi(
        &self,
        repo_ids: &[String],
    ) -> Result<HashMap<String, String>, AppError> {
        let mut commits = HashMap::new();
        for repo_id in repo_ids {
            let repo_model = self.repos.repo.find_by_id(repo_id).await?;
            if let Some(r) = repo_model
                && let Some(head_id) = &r.head_commit_id
            {
                commits.insert(repo_id.clone(), head_id.clone());
            }
        }
        Ok(commits)
    }

    /// Get the root fs_id of a commit by its commit_id.
    pub async fn get_commit_root(
        &self,
        repo_id: &str,
        commit_id: &str,
    ) -> Result<Option<String>, AppError> {
        let commit = self
            .repos
            .commit
            .find_by_repo_and_commit_id(repo_id, commit_id)
            .await?;
        Ok(commit.map(|c| c.root_id))
    }

    /// Recursively collect all fs_ids under a given root fs_id.
    pub async fn collect_fs_ids(
        &self,
        repo_id: &str,
        root_fs_id: &str,
    ) -> Result<HashSet<String>, AppError> {
        let mut collected = HashSet::new();
        let mut stack = vec![root_fs_id.to_string()];
        while let Some(fs_id) = stack.pop() {
            if collected.contains(&fs_id) {
                continue;
            }
            if let Some(fs_obj) = self
                .repos
                .fs_object
                .find_by_repo_and_fs_id(repo_id, &fs_id)
                .await?
            {
                collected.insert(fs_id);
                if fs_obj.obj_type == SEAF_METADATA_TYPE_DIR as i8
                    && let Ok(dir_data) =
                        serde_json::from_str::<base::common::FsDirData>(&fs_obj.data)
                {
                    for entry in &dir_data.dirents {
                        stack.push(entry.id.clone());
                    }
                }
            }
        }
        Ok(collected)
    }

    /// Filter fs_ids to only include directory objects.
    pub async fn filter_dir_ids(
        &self,
        repo_id: &str,
        ids: &HashSet<String>,
    ) -> Result<Vec<String>, AppError> {
        let id_list: Vec<String> = ids.iter().cloned().collect();
        let objects = self
            .repos
            .fs_object
            .find_by_repo_and_fs_ids(repo_id, &id_list)
            .await?;
        let dir_set: HashSet<String> = objects
            .iter()
            .filter(|obj| obj.obj_type == SEAF_METADATA_TYPE_DIR as i8)
            .map(|obj| obj.fs_id.clone())
            .collect();
        Ok(ids
            .iter()
            .filter(|id| dir_set.contains(*id))
            .cloned()
            .collect())
    }

    /// Batch fetch FS objects for given fs_ids.
    pub async fn fetch_fs_objects(
        &self,
        repo_id: &str,
        fs_ids: &[String],
    ) -> Result<Vec<infra::entity::fs_object::Model>, AppError> {
        self.repos
            .fs_object
            .find_by_repo_and_fs_ids(repo_id, fs_ids)
            .await
    }

    /// Batch insert FS objects with ON CONFLICT DO NOTHING semantics.
    pub async fn insert_fs_objects(
        &self,
        repo_id: &str,
        entries: Vec<(String, Vec<u8>)>,
    ) -> Result<(), AppError> {
        let models: Vec<infra::entity::fs_object::ActiveModel> = entries
            .into_iter()
            .filter_map(|(fs_id, obj_data)| {
                let decompressed = pack_fs::decompress_fs_data(&obj_data).ok()?;
                let json_str = String::from_utf8(decompressed).ok()?;
                let json_val: serde_json::Value = serde_json::from_str(&json_str).ok()?;
                let obj_type = json_val.get("type").and_then(|v| v.as_i64()).unwrap_or(1) as i8;
                Some(infra::entity::fs_object::ActiveModel {
                    id: sea_orm::NotSet,
                    repo_id: sea_orm::Set(repo_id.to_string()),
                    fs_id: sea_orm::Set(fs_id),
                    obj_type: sea_orm::Set(obj_type),
                    data: sea_orm::Set(json_str),
                })
            })
            .collect();

        if models.is_empty() {
            return Ok(());
        }
        self.repos.fs_object.insert_many(models).await?;
        Ok(())
    }

    /// Insert a commit if it doesn't already exist.
    pub async fn put_commit(&self, data: &base::common::CommitData) -> Result<(), AppError> {
        let existing = self
            .repos
            .commit
            .find_by_repo_and_commit_id(&data.repo_id, &data.commit_id)
            .await?;
        if existing.is_none() {
            let model = infra::entity::commit::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: sea_orm::Set(data.repo_id.clone()),
                commit_id: sea_orm::Set(data.commit_id.clone()),
                root_id: sea_orm::Set(data.root_id.clone()),
                parent_id: sea_orm::Set(data.parent_id.clone()),
                second_parent_id: sea_orm::Set(data.second_parent_id.clone()),
                creator_name: sea_orm::Set(data.creator_name.clone()),
                description: sea_orm::Set(data.description.clone()),
                ctime: sea_orm::Set(data.ctime),
                version: sea_orm::Set(data.version as i8),
            };
            self.repos.commit.insert(model).await?;
        }
        Ok(())
    }

    /// Find a commit by repo_id and commit_id.
    pub async fn find_commit(
        &self,
        repo_id: &str,
        commit_id: &str,
    ) -> Result<Option<infra::entity::commit::Model>, AppError> {
        self.repos
            .commit
            .find_by_repo_and_commit_id(repo_id, commit_id)
            .await
    }

    /// Check if a repo exists by ID.
    pub async fn repo_exists(&self, repo_id: &str) -> Result<bool, AppError> {
        Ok(self.repos.repo.find_by_id(repo_id).await?.is_some())
    }

    /// Find a repo model by ID.
    pub async fn find_repo(
        &self,
        repo_id: &str,
    ) -> Result<Option<infra::entity::repo::Model>, AppError> {
        self.repos.repo.find_by_id(repo_id).await
    }

    /// Update repo head commit.
    pub async fn update_head_commit(
        &self,
        repo_id: &str,
        head_commit_id: Option<String>,
    ) -> Result<(), AppError> {
        self.repos
            .repo
            .update_head_commit(repo_id, head_commit_id)
            .await
    }

    /// Check if a repo is a wiki repo.
    pub async fn is_wiki_repo(&self, repo_id: &str) -> Result<bool, AppError> {
        Ok(self.repos.wiki.find_by_repo_id(repo_id).await?.is_some())
    }

    /// Check if a parent commit exists (not the null commit).
    pub async fn parent_commit_exists(
        &self,
        repo_id: &str,
        parent_id: &str,
    ) -> Result<bool, AppError> {
        if parent_id == "0000000000000000000000000000000000000000" {
            return Ok(true);
        }
        Ok(self
            .repos
            .commit
            .find_by_repo_and_commit_id(repo_id, parent_id)
            .await?
            .is_some())
    }

    async fn get_or_create_sync_token(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<String, AppError> {
        if let Some(existing) = self
            .repos
            .sync_token
            .find_by_repo_and_user(repo_id, user_id)
            .await?
        {
            return Ok(existing.token);
        }

        let token_value = generate_sync_token();
        let now = chrono::Utc::now().timestamp();
        self.repos
            .sync_token
            .create(repo_id, user_id, token_value.clone(), None, now)
            .await?;

        Ok(token_value)
    }
}

/// Response type for accessible repos.
#[derive(serde::Serialize)]
pub struct AccessibleRepo {
    pub repo_id: String,
    pub repo_name: String,
    pub repo_desc: String,
    pub owner_email: String,
    pub token: String,
    pub permission: String,
}

/// Result of a folder permission check.
pub struct FolderPermResult {
    pub valid: bool,
    pub permission: String,
}
