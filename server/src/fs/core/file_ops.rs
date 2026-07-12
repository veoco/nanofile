use crate::domain;
use crate::repository::Repositories;
use base::common::{DirEntryData, FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use infra::crypto::random_key::encrypt_block;
use infra::entity::{commit, repo};
use infra::events;
use infra::storage::DynBlockStorage;
use sea_orm::DatabaseConnection;

/// Sentinel value indicating that no ancestor chain was pre-computed.
/// Callers that don't have an ancestor chain pass this to
/// `update_dir_tree_and_commit` / `update_dir_tree_no_commit`;
/// the walk_up_ancestors function will fall back to on-demand
/// path resolution for each ancestor level.
pub(crate) const EMPTY_ANCESTOR_CHAIN: &[(String, String)] = &[];

pub struct FileOps;

impl FileOps {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_file(
        db: &DatabaseConnection,
        repos: &Repositories,
        repo_id: &str,
        parent_path: &str,
        name: &str,
        data: &[u8],
        modifier: &str,
        replace: bool,
        block_store: &DynBlockStorage,
        // Optional encryption key (key, iv) — when set, blocks are encrypted
        // before writing. Used for encrypted repos during web upload.
        enc_key: Option<(&[u8], &[u8])>,
    ) -> Result<String, AppError> {
        let now = chrono::Utc::now().timestamp();

        // Validate input — name may contain '/' for nested paths.
        base::sanitize::validate_path(parent_path)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        base::sanitize::validate_name(name).map_err(|e| AppError::BadRequest(e.to_string()))?;

        let file_chunks = infra::storage::cdc::file_chunk_cdc(data);

        let mut block_ids = Vec::new();
        let mut total_size: i64 = 0;

        for (offset, size) in &file_chunks {
            let chunk_data = &data[*offset..*offset + size];
            // If encryption key is provided, encrypt the chunk before writing.
            // Seafile encrypts each block individually with a per-block random IV.
            let chunk_to_write = if let Some((key, iv)) = enc_key {
                encrypt_block(chunk_data, key, iv)
            } else {
                chunk_data.to_vec()
            };
            let block_id = block_store.write_block(&chunk_to_write).await?;
            block_ids.push(block_id);
            total_size += *size as i64;
        }

        let file_fs_data = FsFileData {
            block_ids: block_ids.clone(),
            size: total_size,
            obj_type: 1,
            version: 1,
        };
        let file_fs_id = crate::fs::core::store_fs_file_object(db, repo_id, &file_fs_data).await?;

        // Resolve the parent directory and build an ancestor chain for
        // walk_up_ancestors to avoid O(d²) re-resolution.
        let (parent_fs_id, ancestor_chain) = if parent_path == "/" {
            // Find root via repo head commit, or create empty root fs_object for empty repo
            let repo_model = repos.repo.find_by_id(repo_id).await?;
            if let Some(commit_id) = repo_model.as_ref().and_then(|r| r.head_commit_id.clone()) {
                let commit_ent = repos
                    .commit
                    .find_by_repo_and_commit_id(repo_id, &commit_id)
                    .await?
                    .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;
                (commit_ent.root_id, Vec::new())
            } else {
                let empty_dir = FsDirData {
                    dirents: vec![],
                    obj_type: SEAF_METADATA_TYPE_DIR,
                    version: 1,
                };
                (
                    crate::fs::core::store_fs_dir_object(db, repo_id, &empty_dir).await?,
                    Vec::new(),
                )
            }
        } else {
            Self::resolve_fs_id_chain(repos, repo_id, parent_path).await?
        };

        let parent_data = Self::read_dir_fs_object(repos, repo_id, &parent_fs_id).await?;

        let mut dirents = parent_data.dirents;

        // If replacing, remove any existing entry with the same name.
        if replace {
            dirents.retain(|d| d.name != name);
        }

        dirents.push(DirEntryData {
            id: file_fs_id.clone(),
            mode: infra::serialization::S_IFREG,
            modifier: modifier.to_string(),
            mtime: now,
            name: name.to_string(),
            size: total_size,
        });

        let new_dir_data = FsDirData {
            dirents,
            obj_type: SEAF_METADATA_TYPE_DIR,
            version: 1,
        };
        let new_dir_fs_id =
            crate::fs::core::store_fs_dir_object(db, repo_id, &new_dir_data).await?;

        // Walk up to root, updating all ancestor directories
        let root_fs_id = if parent_path == "/" {
            new_dir_fs_id.clone()
        } else {
            Self::walk_up_ancestors(
                repos,
                db,
                repo_id,
                parent_path,
                &new_dir_fs_id,
                &ancestor_chain,
            )
            .await?
        };

        let repo_model = repos.repo.find_by_id(repo_id).await?;
        let parent_commit_id = repo_model.as_ref().and_then(|r| r.head_commit_id.clone());

        let commit_data = base::common::CommitData {
            commit_id: String::new(),
            repo_id: repo_id.to_string(),
            root_id: root_fs_id.clone(),
            creator_name: modifier.to_string(),
            creator: "0000000000000000000000000000000000000000".to_string(),
            description: format!("Added {}", name),
            ctime: now,
            parent_id: parent_commit_id.clone(),
            second_parent_id: None,
            repo_name: None,
            repo_desc: None,
            repo_category: None,
            encrypted: None,
            enc_version: None,
            magic: None,
            key: None,
            version: 1,
        };
        let commit_id = domain::commit::compute_commit_id(&commit_data);

        let commit_model = commit::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: sea_orm::Set(repo_id.to_string()),
            commit_id: sea_orm::Set(commit_id.clone()),
            root_id: sea_orm::Set(root_fs_id),
            parent_id: sea_orm::Set(parent_commit_id),
            second_parent_id: sea_orm::NotSet,
            creator_name: sea_orm::Set(modifier.to_string()),
            description: sea_orm::Set(format!("Added {}", name)),
            ctime: sea_orm::Set(now),
            version: sea_orm::Set(1i8),
        };
        repos.commit.insert(commit_model).await?;

        let repo = repo_model.ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        let mut repo_active: repo::ActiveModel = repo.into();
        repo_active.head_commit_id = sea_orm::Set(Some(commit_id.clone()));
        repo_active.updated_at = sea_orm::Set(now);
        repos.repo.update(repo_active).await?;

        // Fire repo-update notification through the global broadcast channel.
        // Without this, the Seafile client won't know about the new file until
        // its next poll cycle, causing a noticeable sync delay.
        events::publish_repo_update(repo_id, commit_id);

        Ok(file_fs_id)
    }

    /// Walk up the directory tree from immediate_parent_path to root,
    /// updating each ancestor's FsDirData to reference the new child fs_id.
    /// Returns the new root fs_id.
    ///
    /// `ancestor_chain` is an optional list of `(path, fs_id)` pairs for
    /// intermediate directories, ordered from root down to
    /// `immediate_parent_path`'s parent. When provided, `resolve_fs_id` is
    /// avoided for each level, reducing O(d²) resolve to O(d).
    pub(crate) async fn walk_up_ancestors(
        repos: &Repositories,
        db: &DatabaseConnection,
        repo_id: &str,
        immediate_parent_path: &str,
        new_immediate_parent_fs_id: &str,
        ancestor_chain: &[(String, String)],
    ) -> Result<String, AppError> {
        let mut current_child_fs_id = new_immediate_parent_fs_id.to_string();
        let mut current_child_path = immediate_parent_path.to_string();

        // Build a path→fs_id map from the ancestor chain for O(1) lookups.
        // When the chain is empty (caller didn't provide one), falls through
        // to on-demand resolve_fs_id for each ancestor level.
        let chain_map: std::collections::HashMap<&str, &str> = ancestor_chain
            .iter()
            .map(|(p, id)| (p.as_str(), id.as_str()))
            .collect();

        loop {
            // Split into parent path and child name
            let (parent_path, child_name) = match current_child_path.rsplit_once('/') {
                Some(("", name)) => ("/".to_string(), name.to_string()),
                Some((parent, name)) => (parent.to_string(), name.to_string()),
                None => {
                    // current_child_path is "/" — we are at root
                    return Ok(current_child_fs_id);
                }
            };

            // Find the ancestor's current fs_id.
            // When ancestor_chain was provided, look up the parent_path
            // from the chain instead of re-resolving from root.
            let ancestor_fs_id = if parent_path == "/" {
                Self::resolve_root_fs_id(repos, repo_id).await?
            } else {
                match chain_map.get(parent_path.as_str()) {
                    Some(id) => id.to_string(),
                    None => Self::resolve_fs_id(repos, repo_id, &parent_path).await?,
                }
            };

            // Read ancestor's FsDirData
            let mut ancestor_data =
                Self::read_dir_fs_object(repos, repo_id, &ancestor_fs_id).await?;

            // Find and update the child entry, or add if not present
            let mut found = false;
            for entry in &mut ancestor_data.dirents {
                if entry.name == child_name {
                    entry.id = current_child_fs_id.clone();
                    found = true;
                    break;
                }
            }

            if !found {
                // Child was created by create_dir which always updates FsDirData
                // now — this branch should not be reached. Fall back gracefully
                // with S_IFDIR defaults.
                ancestor_data.dirents.push(DirEntryData {
                    id: current_child_fs_id.clone(),
                    mode: infra::serialization::S_IFDIR,
                    modifier: String::new(),
                    mtime: chrono::Utc::now().timestamp(),
                    name: child_name.clone(),
                    size: 0,
                });
            }

            // Create new fs_object for ancestor
            let new_ancestor_fs_id =
                crate::fs::core::store_fs_dir_object(db, repo_id, &ancestor_data).await?;

            // If we reached root, return
            if parent_path == "/" {
                return Ok(new_ancestor_fs_id);
            }

            // Move up
            current_child_fs_id = new_ancestor_fs_id;
            current_child_path = parent_path;
        }
    }

    /// Find the root fs_id via the repo's head commit.
    async fn resolve_root_fs_id(repos: &Repositories, repo_id: &str) -> Result<String, AppError> {
        let repo_model = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        let head_commit_id = repo_model
            .head_commit_id
            .ok_or_else(|| AppError::NotFound("repo has no head commit".into()))?;
        let commit_ent = repos
            .commit
            .find_by_repo_and_commit_id(repo_id, &head_commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;
        Ok(commit_ent.root_id)
    }

    /// Resolve a path to its fs_id by walking the FS tree from root.
    async fn resolve_fs_id(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
    ) -> Result<String, AppError> {
        if path == "/" {
            return Self::resolve_root_fs_id(repos, repo_id).await;
        }

        let root_fs_id = Self::resolve_root_fs_id(repos, repo_id).await?;
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let mut current_fs_id = root_fs_id;

        for part in parts {
            let dir_data = Self::read_dir_fs_object(repos, repo_id, &current_fs_id).await?;
            let found = dir_data
                .dirents
                .iter()
                .find(|e| e.name == part)
                .ok_or_else(|| {
                    AppError::NotFound(format!("path component '{}' not found in '{}'", part, path))
                })?;
            current_fs_id = found.id.clone();
        }

        Ok(current_fs_id)
    }

    /// Resolve a path to its fs_id, also returning the fs_id of every
    /// intermediate directory from root down to the last component.
    ///
    /// The returned vector is ordered from root outward, e.g. for path
    /// `"/a/b/c"`: `[("/a", fs_a), ("/a/b", fs_b), ("/a/b/c", fs_c)]`.
    ///
    /// This can be passed to `walk_up_ancestors` to avoid re-resolving
    /// each ancestor from root, reducing O(d²) to O(d).
    pub(crate) async fn resolve_fs_id_chain(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
    ) -> Result<(String, Vec<(String, String)>), AppError> {
        let root_fs_id = Self::resolve_root_fs_id(repos, repo_id).await?;
        if path == "/" || path.is_empty() {
            return Ok((root_fs_id, Vec::new()));
        }

        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let mut current_fs_id = root_fs_id;
        let mut chain = Vec::with_capacity(parts.len());
        let mut accumulated = String::new();

        for part in &parts {
            accumulated.push('/');
            accumulated.push_str(part);

            let dir_data = Self::read_dir_fs_object(repos, repo_id, &current_fs_id).await?;
            let found = dir_data
                .dirents
                .iter()
                .find(|e| e.name == *part)
                .ok_or_else(|| {
                    AppError::NotFound(format!("path component '{}' not found in '{}'", part, path))
                })?;
            current_fs_id = found.id.clone();
            chain.push((accumulated.clone(), current_fs_id.clone()));
        }

        Ok((current_fs_id, chain))
    }

    /// Apply a transformation to a parent directory's FsDirData entries,
    /// update the FS tree (walk up ancestors to root), create a new commit,
    /// and update the repo HEAD.
    ///
    /// Parameters:
    /// - `parent_path`: path of the parent directory (e.g. "/dir", "/")
    /// - `parent_fs_id`: the parent directory's fs_id **before** modification
    /// - `update_fn`: closure that modifies `&mut Vec<DirEntryData>` in-place
    /// - `description`: commit description
    ///
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn update_dir_tree_and_commit(
        db: &DatabaseConnection,
        repos: &Repositories,
        repo_id: &str,
        parent_path: &str,
        parent_fs_id: &str,
        modifier: &str,
        description: &str,
        ancestor_chain: &[(String, String)],
        update_fn: impl FnOnce(&mut Vec<DirEntryData>) -> Result<(), AppError>,
    ) -> Result<String, AppError> {
        let mut parent_data = Self::read_dir_fs_object(repos, repo_id, parent_fs_id).await?;
        update_fn(&mut parent_data.dirents)?;

        let new_parent_fs_id =
            crate::fs::core::store_fs_dir_object(db, repo_id, &parent_data).await?;

        let root_fs_id = if parent_path == "/" {
            new_parent_fs_id.clone()
        } else {
            Self::walk_up_ancestors(
                repos,
                db,
                repo_id,
                parent_path,
                &new_parent_fs_id,
                ancestor_chain,
            )
            .await?
        };

        Self::create_commit(repos, repo_id, &root_fs_id, modifier, description).await?;

        Ok(root_fs_id)
    }

    /// Apply a transformation to a parent directory's FsDirData entries,
    /// update the FS tree (walk up ancestors to root), but do NOT create
    /// a commit. Returns the new root_fs_id.
    ///
    /// Useful for multi-step operations (e.g. move) where the caller wants
    /// to update several parts of the tree before creating a single commit.
    pub(crate) async fn update_dir_tree_no_commit(
        db: &DatabaseConnection,
        repos: &Repositories,
        repo_id: &str,
        parent_path: &str,
        parent_fs_id: &str,
        ancestor_chain: &[(String, String)],
        update_fn: impl FnOnce(&mut Vec<DirEntryData>) -> Result<(), AppError>,
    ) -> Result<String, AppError> {
        let mut parent_data = Self::read_dir_fs_object(repos, repo_id, parent_fs_id).await?;
        update_fn(&mut parent_data.dirents)?;

        let new_parent_fs_id =
            crate::fs::core::store_fs_dir_object(db, repo_id, &parent_data).await?;

        let root_fs_id = if parent_path == "/" {
            new_parent_fs_id.clone()
        } else {
            Self::walk_up_ancestors(
                repos,
                db,
                repo_id,
                parent_path,
                &new_parent_fs_id,
                ancestor_chain,
            )
            .await?
        };

        Ok(root_fs_id)
    }

    /// Create a commit with the given root_fs_id and update the repo's HEAD.
    pub(crate) async fn create_commit(
        repos: &Repositories,
        repo_id: &str,
        root_fs_id: &str,
        creator_name: &str,
        description: &str,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();

        let repo_model = repos.repo.find_by_id(repo_id).await?;
        let parent_commit_id = repo_model.as_ref().and_then(|r| r.head_commit_id.clone());

        let commit_data = base::common::CommitData {
            commit_id: String::new(),
            repo_id: repo_id.to_string(),
            root_id: root_fs_id.to_string(),
            creator_name: creator_name.to_string(),
            creator: "0000000000000000000000000000000000000000".to_string(),
            description: description.to_string(),
            ctime: now,
            parent_id: parent_commit_id.clone(),
            second_parent_id: None,
            repo_name: None,
            repo_desc: None,
            repo_category: None,
            encrypted: None,
            enc_version: None,
            magic: None,
            key: None,
            version: 1,
        };
        let commit_id = domain::commit::compute_commit_id(&commit_data);

        let commit_model = commit::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: sea_orm::Set(repo_id.to_string()),
            commit_id: sea_orm::Set(commit_id.clone()),
            root_id: sea_orm::Set(root_fs_id.to_string()),
            parent_id: sea_orm::Set(parent_commit_id),
            second_parent_id: sea_orm::NotSet,
            creator_name: sea_orm::Set(creator_name.to_string()),
            description: sea_orm::Set(description.to_string()),
            ctime: sea_orm::Set(now),
            version: sea_orm::Set(1i8),
        };
        repos.commit.insert(commit_model).await?;

        let repo = repo_model.ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        let mut repo_active: repo::ActiveModel = repo.into();
        let commit_id_clone = commit_id.clone();
        repo_active.head_commit_id = sea_orm::Set(Some(commit_id));
        repo_active.updated_at = sea_orm::Set(now);
        repos.repo.update(repo_active).await?;

        // Fire repo-update notification through the global broadcast channel.
        events::publish_repo_update(repo_id, commit_id_clone);

        Ok(())
    }

    pub async fn read_dir_fs_object(
        repos: &Repositories,
        repo_id: &str,
        fs_id: &str,
    ) -> Result<FsDirData, AppError> {
        crate::fs::core::read_fs_dir_data(repos, repo_id, fs_id).await
    }

    pub async fn read_file_fs_object(
        repos: &Repositories,
        repo_id: &str,
        fs_id: &str,
    ) -> Result<FsFileData, AppError> {
        crate::fs::core::read_fs_file_data(repos, repo_id, fs_id).await
    }
}
