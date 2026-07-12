use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rand::RngExt;
use sea_orm::DatabaseConnection;

use crate::indexer::TextIndexer;
use crate::repository::Repositories;
use crate::service::auth::token::generate_sync_token;
use base::common::SEAF_METADATA_TYPE_DIR;
use base::error::AppError;
use infra::serialization::pack_fs;
use infra::storage::DynBlockStorage;

/// Service for sync protocol operations.
pub struct SyncService {
    pub(super) repos: Arc<Repositories>,
    pub(super) db: Arc<DatabaseConnection>,
    pub(super) block_store: DynBlockStorage,
    pub(super) indexer: Option<TextIndexer>,
}

impl SyncService {
    pub fn new(
        repos: Arc<Repositories>,
        db: Arc<DatabaseConnection>,
        block_store: DynBlockStorage,
        indexer: Option<TextIndexer>,
    ) -> Self {
        Self {
            repos,
            db,
            block_store,
            indexer,
        }
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
        use crate::repository::commit::CreateCommitParams;
        let existing = self
            .repos
            .commit
            .find_by_repo_and_commit_id(&data.repo_id, &data.commit_id)
            .await?;
        if existing.is_none() {
            self.repos
                .commit
                .insert_commit(CreateCommitParams {
                    repo_id: data.repo_id.clone(),
                    commit_id: data.commit_id.clone(),
                    root_id: data.root_id.clone(),
                    parent_id: data.parent_id.clone(),
                    second_parent_id: data.second_parent_id.clone(),
                    creator_name: data.creator_name.clone(),
                    description: data.description.clone(),
                    ctime: data.ctime,
                    version: data.version as i8,
                })
                .await?;
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

    // ── Branch update (sync protocol) ──────────────────────────────────

    /// Check file blocks exist for a commit and compute size delta.
    pub async fn check_commit_blocks(
        &self,
        repo_id: &str,
        new_root_id: &str,
        base_root_id: Option<&str>,
        missing: &mut Vec<String>,
        size_delta: &mut i64,
    ) -> Result<(), AppError> {
        if new_root_id == "0000000000000000000000000000000000000000" {
            return Ok(());
        }

        if let Some(base_root) = base_root_id {
            if base_root == new_root_id {
                return Ok(());
            }

            struct DiffFrame {
                base_fs_id: Option<String>,
                new_fs_id: String,
                prefix: String,
            }

            let mut stack: Vec<DiffFrame> = vec![DiffFrame {
                base_fs_id: Some(base_root.to_string()),
                new_fs_id: new_root_id.to_string(),
                prefix: String::new(),
            }];

            while let Some(frame) = stack.pop() {
                let Some(ref base_fs) = frame.base_fs_id else {
                    let new_dir = match crate::fs::core::read_fs_dir_data(
                        &self.repos,
                        repo_id,
                        &frame.new_fs_id,
                    )
                    .await
                    {
                        Ok(d) => d,
                        Err(_) => continue,
                    };
                    for entry in &new_dir.dirents {
                        let child = if frame.prefix.is_empty() {
                            entry.name.clone()
                        } else {
                            format!("{}/{}", frame.prefix, entry.name)
                        };
                        if entry.mode & infra::serialization::S_IFDIR != 0 {
                            stack.push(DiffFrame {
                                base_fs_id: None,
                                new_fs_id: entry.id.clone(),
                                prefix: child,
                            });
                        } else {
                            *size_delta += entry.size;
                            self.check_file_blocks(repo_id, &entry.id, &child, missing)
                                .await?;
                        }
                    }
                    continue;
                };

                if *base_fs == frame.new_fs_id {
                    continue;
                }
                if *base_fs == infra::common::EMPTY_SHA1 {
                    stack.push(DiffFrame {
                        base_fs_id: None,
                        new_fs_id: frame.new_fs_id,
                        prefix: frame.prefix,
                    });
                    continue;
                }

                let base_dir =
                    match crate::fs::core::read_fs_dir_data(&self.repos, repo_id, base_fs).await {
                        Ok(d) => d,
                        Err(_) => continue,
                    };
                let new_dir =
                    match crate::fs::core::read_fs_dir_data(&self.repos, repo_id, &frame.new_fs_id)
                        .await
                    {
                        Ok(d) => d,
                        Err(_) => continue,
                    };

                let base_map: HashMap<&str, &base::common::DirEntryData> = base_dir
                    .dirents
                    .iter()
                    .map(|d| (d.name.as_str(), d))
                    .collect();

                for new_entry in &new_dir.dirents {
                    let child = if frame.prefix.is_empty() {
                        new_entry.name.clone()
                    } else {
                        format!("{}/{}", frame.prefix, new_entry.name)
                    };
                    let is_dir = new_entry.mode & infra::serialization::S_IFDIR != 0;
                    match base_map.get(new_entry.name.as_str()) {
                        None => {
                            if is_dir {
                                stack.push(DiffFrame {
                                    base_fs_id: None,
                                    new_fs_id: new_entry.id.clone(),
                                    prefix: child,
                                });
                            } else {
                                *size_delta += new_entry.size;
                                self.check_file_blocks(repo_id, &new_entry.id, &child, missing)
                                    .await?;
                            }
                        }
                        Some(base_entry) => {
                            if new_entry.id == base_entry.id {
                                continue;
                            }
                            if is_dir && (base_entry.mode & infra::serialization::S_IFDIR != 0) {
                                stack.push(DiffFrame {
                                    base_fs_id: Some(base_entry.id.clone()),
                                    new_fs_id: new_entry.id.clone(),
                                    prefix: child,
                                });
                            } else {
                                *size_delta += new_entry.size - base_entry.size;
                                self.check_file_blocks(repo_id, &new_entry.id, &child, missing)
                                    .await?;
                            }
                        }
                    }
                }
            }
        } else {
            self.full_check_blocks(repo_id, new_root_id, missing, size_delta)
                .await?;
        }

        Ok(())
    }

    async fn full_check_blocks(
        &self,
        repo_id: &str,
        root_id: &str,
        missing: &mut Vec<String>,
        size_total: &mut i64,
    ) -> Result<(), AppError> {
        let mut stack: Vec<(String, String)> = vec![(root_id.to_string(), String::new())];
        while let Some((fs_id, path)) = stack.pop() {
            if fs_id == "0000000000000000000000000000000000000000" {
                continue;
            }
            let obj = match self
                .repos
                .fs_object
                .find_by_repo_and_fs_id(repo_id, &fs_id)
                .await?
            {
                Some(o) => o,
                None => continue,
            };
            if obj.obj_type == 1 {
                let file_data: base::common::FsFileData = serde_json::from_str(&obj.data)
                    .map_err(|e| AppError::Internal(format!("invalid file object: {e}")))?;
                *size_total += file_data.size;
                if self.check_blocks_concurrent(&file_data.block_ids).await {
                    missing.push(path.clone());
                }
            } else if obj.obj_type == 3 {
                let dir_data: base::common::FsDirData = serde_json::from_str(&obj.data)
                    .map_err(|e| AppError::Internal(format!("invalid dir object: {e}")))?;
                for entry in &dir_data.dirents {
                    let child_path = if path.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{}/{}", path, entry.name)
                    };
                    stack.push((entry.id.clone(), child_path));
                }
            }
        }
        Ok(())
    }

    async fn check_file_blocks(
        &self,
        repo_id: &str,
        fs_id: &str,
        path: &str,
        missing: &mut Vec<String>,
    ) -> Result<(), AppError> {
        let obj = match self
            .repos
            .fs_object
            .find_by_repo_and_fs_id(repo_id, fs_id)
            .await?
        {
            Some(o) => o,
            None => return Ok(()),
        };
        if obj.obj_type != 1 {
            return Ok(());
        }
        let file_data: base::common::FsFileData = serde_json::from_str(&obj.data)
            .map_err(|e| AppError::Internal(format!("invalid file object: {e}")))?;
        if self.check_blocks_concurrent(&file_data.block_ids).await {
            missing.push(path.to_string());
        }
        Ok(())
    }

    async fn check_blocks_concurrent(&self, block_ids: &[String]) -> bool {
        const BATCH_SIZE: usize = 8;
        for chunk in block_ids.chunks(BATCH_SIZE) {
            let futures: Vec<_> = chunk
                .iter()
                .map(|block_id| {
                    let store = self.block_store.clone();
                    let id = block_id.clone();
                    async move { !store.has_block(&id).await }
                })
                .collect();
            let results = futures::future::join_all(futures).await;
            if results.into_iter().any(|missing| missing) {
                return true;
            }
        }
        false
    }

    /// Perform the branch update (CAS retry loop). Returns `true` if the commit
    /// was already the current HEAD (no-op).
    pub async fn update_branch(
        &self,
        repo_id: &str,
        new_head: &str,
        user_id: i32,
        commit_desc: &str,
    ) -> Result<bool, AppError> {
        use crate::fs::core::tree_diff;
        use infra::events;

        const EXCLUDED_ACTIVITY_PREFIXES: &[&str] =
            &["/_Internal", "/images/sdoc", "/images/auto-upload"];
        const MAX_BRANCH_RETRY: u32 = 3;

        let new_commit = self
            .find_commit(repo_id, new_head)
            .await?
            .ok_or_else(|| AppError::Internal("commit not found".into()))?;

        let base_root_id: Option<String> = if let Some(ref parent_id) = new_commit.parent_id
            && parent_id != "0000000000000000000000000000000000000000"
        {
            self.find_commit(repo_id, parent_id)
                .await?
                .map(|c| c.root_id)
        } else {
            None
        };

        let mut missing = Vec::new();
        let mut size_delta: i64 = 0;
        self.check_commit_blocks(
            repo_id,
            &new_commit.root_id,
            base_root_id.as_deref(),
            &mut missing,
            &mut size_delta,
        )
        .await?;
        if !missing.is_empty() {
            return Err(AppError::BlockMissing);
        }

        infra::storage::check_commit_file_locks(
            self.db.as_ref(),
            repo_id,
            &new_commit.root_id,
            user_id,
        )
        .await?;

        if let Some(ref parent_id) = new_commit.parent_id
            && !self.parent_commit_exists(repo_id, parent_id).await?
        {
            return Err(AppError::BadRequest("parent commit not found".into()));
        }

        let mut attempt: u32 = 0;

        loop {
            attempt += 1;

            let current_head = self
                .find_repo(repo_id)
                .await?
                .ok_or_else(|| AppError::Internal("repo not found".into()))?
                .head_commit_id;

            let is_same_commit = current_head.as_deref() == Some(new_head);
            if is_same_commit {
                events::publish_repo_update(repo_id, new_head.to_string());
                return Ok(true);
            }

            if current_head.is_some() && new_commit.parent_id != current_head {
                if attempt < MAX_BRANCH_RETRY {
                    let delay_ms = rand::rng().random_range(100..=500);
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    continue;
                }
                return Err(AppError::Conflict(
                    "commit parent_id does not match current HEAD".into(),
                ));
            }

            self.update_head_commit(repo_id, Some(new_head.to_string()))
                .await?;

            crate::fs::core::adjust_repo_size(self.db.as_ref(), &self.repos, repo_id, size_delta)
                .await?;

            let is_wiki = self.is_wiki_repo(repo_id).await?;

            let old_root = if let Some(ref parent_id) = new_commit.parent_id
                && parent_id != "0000000000000000000000000000000000000000"
            {
                self.find_commit(repo_id, parent_id)
                    .await?
                    .map(|c| c.root_id)
            } else {
                None
            };

            let mut changes = tree_diff::diff_trees(
                &self.repos,
                repo_id,
                old_root.as_deref(),
                &new_commit.root_id,
            )
            .await
            .unwrap_or_default();

            if !is_wiki {
                let is_reverted =
                    commit_desc.starts_with("Reverted") || commit_desc.starts_with("Recovered");

                for change in &mut changes {
                    if is_reverted && (change.op_type == "create" || change.op_type == "edit") {
                        change.op_type = "recover";
                    }

                    if EXCLUDED_ACTIVITY_PREFIXES
                        .iter()
                        .any(|p| change.path.starts_with(p))
                    {
                        continue;
                    }

                    infra::activity_log::log_activity(
                        self.db.as_ref(),
                        repo_id,
                        change.op_type,
                        change.obj_type,
                        &change.path,
                        user_id,
                        change.old_path.as_deref(),
                        Some(change.size),
                        Some(&change.obj_id),
                        None,
                        None,
                    )
                    .await;
                }
            }

            if let Some(ref indexer) = self.indexer {
                for change in &changes {
                    if change.obj_type != "file" {
                        continue;
                    }
                    match change.op_type {
                        "create" | "edit" | "recover" => {
                            if let Err(e) = indexer
                                .reindex_file(
                                    self.db.as_ref(),
                                    repo_id,
                                    &change.path,
                                    &self.block_store,
                                )
                                .await
                            {
                                tracing::warn!("sync index file {}: {e}", change.path);
                            }
                        }
                        "delete" => {
                            if let Err(e) = indexer.delete_file(repo_id, &change.path) {
                                tracing::warn!("sync delete index {}: {e}", change.path);
                            }
                        }
                        "rename" | "move" => {
                            if let Some(ref old_path) = change.old_path {
                                let _ = indexer.delete_file(repo_id, old_path);
                            }
                            if let Err(e) = indexer
                                .reindex_file(
                                    self.db.as_ref(),
                                    repo_id,
                                    &change.path,
                                    &self.block_store,
                                )
                                .await
                            {
                                tracing::warn!("sync reindex {}: {e}", change.path);
                            }
                        }
                        _ => {}
                    }
                }
            }

            events::publish_repo_update(repo_id, new_head.to_string());
            return Ok(false);
        }
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
