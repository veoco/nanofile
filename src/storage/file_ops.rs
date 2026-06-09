use crate::entity::{commit, repo};
use crate::notification::events_channel;
use crate::serialization::fs_json::{DirEntryData, FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR};
use crate::storage::DynBlockStorage;
use crate::storage::path_cache::PathCache;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

pub struct FileOps;

impl FileOps {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_file(
        db: &DatabaseConnection,
        repo_id: &str,
        parent_path: &str,
        name: &str,
        data: &[u8],
        modifier: &str,
        replace: bool,
        block_store: &DynBlockStorage,
        path_cache: Option<&PathCache>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let now = chrono::Utc::now().timestamp();

        let file_chunks = crate::storage::cdc::file_chunk_cdc(data);

        let mut block_ids = Vec::new();
        let mut total_size: i64 = 0;

        for (offset, size) in &file_chunks {
            let chunk_data = &data[*offset..*offset + size];
            let block_id = block_store.write_block(chunk_data).await?;
            block_ids.push(block_id);
            total_size += *size as i64;
        }

        let file_fs_data = FsFileData {
            block_ids: block_ids.clone(),
            size: total_size,
            obj_type: 1,
            version: 1,
        };
        let file_fs_id = file_fs_data.compute_and_store(db, repo_id).await?;

        let parent_fs_id = if parent_path == "/" {
            // Find root via repo head commit, or create empty root fs_object for empty repo
            let repo_model = repo::Entity::find_by_id(repo_id).one(db).await?;
            if let Some(commit_id) = repo_model.as_ref().and_then(|r| r.head_commit_id.clone()) {
                let commit_ent = commit::Entity::find()
                    .filter(commit::Column::CommitId.eq(&commit_id))
                    .one(db)
                    .await?
                    .ok_or_else(|| Box::<dyn std::error::Error>::from("head commit not found"))?;
                commit_ent.root_id
            } else {
                let empty_dir = FsDirData {
                    dirents: vec![],
                    obj_type: SEAF_METADATA_TYPE_DIR,
                    version: 1,
                };
                empty_dir.compute_and_store(db, repo_id).await?
            }
        } else {
            Self::resolve_fs_id(db, repo_id, parent_path).await?
        };

        let parent_data = Self::read_dir_fs_object(db, repo_id, &parent_fs_id).await?;

        let mut dirents = parent_data.dirents;

        // If replacing, remove any existing entry with the same name.
        if replace {
            dirents.retain(|d| d.name != name);
        }

        dirents.push(DirEntryData {
            id: file_fs_id.clone(),
            mode: crate::serialization::S_IFREG,
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
        let new_dir_fs_id = new_dir_data.compute_and_store(db, repo_id).await?;

        // Walk up to root, updating all ancestor directories
        let root_fs_id = if parent_path == "/" {
            new_dir_fs_id.clone()
        } else {
            Self::walk_up_ancestors(db, repo_id, parent_path, &new_dir_fs_id).await?
        };

        let repo_model = repo::Entity::find_by_id(repo_id).one(db).await?;
        let parent_commit_id = repo_model.as_ref().and_then(|r| r.head_commit_id.clone());

        let commit_data = crate::serialization::commit_json::CommitData {
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
        let commit_id = commit_data.compute_commit_id();

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
        commit::Entity::insert(commit_model).exec(db).await?;

        let mut repo_active: repo::ActiveModel = repo_model.unwrap().into();
        repo_active.head_commit_id = sea_orm::Set(Some(commit_id.clone()));
        repo_active.updated_at = sea_orm::Set(now);
        repo_active.update(db).await?;

        if let Some(cache) = path_cache {
            cache.clear_repo(repo_id);
        }

        // Fire repo-update notification through the global broadcast channel.
        // Without this, the Seafile client won't know about the new file until
        // its next poll cycle, causing a noticeable sync delay.
        events_channel::publish_repo_update(repo_id, commit_id);

        Ok(file_fs_id)
    }

    /// Walk up the directory tree from immediate_parent_path to root,
    /// updating each ancestor's FsDirData to reference the new child fs_id.
    /// Returns the new root fs_id.
    pub(crate) async fn walk_up_ancestors(
        db: &DatabaseConnection,
        repo_id: &str,
        immediate_parent_path: &str,
        new_immediate_parent_fs_id: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut current_child_fs_id = new_immediate_parent_fs_id.to_string();
        let mut current_child_path = immediate_parent_path.to_string();

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

            // Find the ancestor's current fs_id by resolving via the FS tree
            let ancestor_fs_id = if parent_path == "/" {
                Self::resolve_root_fs_id(db, repo_id).await?
            } else {
                Self::resolve_fs_id(db, repo_id, &parent_path).await?
            };

            // Read ancestor's FsDirData
            let mut ancestor_data = Self::read_dir_fs_object(db, repo_id, &ancestor_fs_id).await?;

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
                    mode: crate::serialization::S_IFDIR,
                    modifier: String::new(),
                    mtime: chrono::Utc::now().timestamp(),
                    name: child_name.clone(),
                    size: 0,
                });
            }

            // Create new fs_object for ancestor
            let new_ancestor_fs_id = ancestor_data.compute_and_store(db, repo_id).await?;

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
    async fn resolve_root_fs_id(
        db: &DatabaseConnection,
        repo_id: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let repo_model = repo::Entity::find_by_id(repo_id)
            .one(db)
            .await?
            .ok_or_else(|| "repo not found".to_string())?;
        let head_commit_id = repo_model
            .head_commit_id
            .ok_or_else(|| "repo has no head commit".to_string())?;
        let commit_ent = commit::Entity::find()
            .filter(commit::Column::CommitId.eq(&head_commit_id))
            .one(db)
            .await?
            .ok_or_else(|| "head commit not found".to_string())?;
        Ok(commit_ent.root_id)
    }

    /// Resolve a path to its fs_id by walking the FS tree from root.
    async fn resolve_fs_id(
        db: &DatabaseConnection,
        repo_id: &str,
        path: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        if path == "/" {
            return Self::resolve_root_fs_id(db, repo_id).await;
        }

        let root_fs_id = Self::resolve_root_fs_id(db, repo_id).await?;
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let mut current_fs_id = root_fs_id;

        for part in parts {
            let dir_data = Self::read_dir_fs_object(db, repo_id, &current_fs_id).await?;
            let found = dir_data
                .dirents
                .iter()
                .find(|e| e.name == part)
                .ok_or_else(|| format!("path component '{}' not found in '{}'", part, path))?;
            current_fs_id = found.id.clone();
        }

        Ok(current_fs_id)
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
        repo_id: &str,
        parent_path: &str,
        parent_fs_id: &str,
        modifier: &str,
        description: &str,
        path_cache: Option<&PathCache>,
        update_fn: impl FnOnce(&mut Vec<DirEntryData>) -> Result<(), Box<dyn std::error::Error>>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut parent_data = Self::read_dir_fs_object(db, repo_id, parent_fs_id).await?;
        update_fn(&mut parent_data.dirents)?;

        let new_parent_fs_id = parent_data.compute_and_store(db, repo_id).await?;

        let root_fs_id = if parent_path == "/" {
            new_parent_fs_id.clone()
        } else {
            Self::walk_up_ancestors(db, repo_id, parent_path, &new_parent_fs_id).await?
        };

        Self::create_commit(db, repo_id, &root_fs_id, modifier, description, path_cache).await?;

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
        repo_id: &str,
        parent_path: &str,
        parent_fs_id: &str,
        path_cache: Option<&PathCache>,
        update_fn: impl FnOnce(&mut Vec<DirEntryData>) -> Result<(), Box<dyn std::error::Error>>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut parent_data = Self::read_dir_fs_object(db, repo_id, parent_fs_id).await?;
        update_fn(&mut parent_data.dirents)?;

        let new_parent_fs_id = parent_data.compute_and_store(db, repo_id).await?;

        let root_fs_id = if parent_path == "/" {
            new_parent_fs_id.clone()
        } else {
            Self::walk_up_ancestors(db, repo_id, parent_path, &new_parent_fs_id).await?
        };

        if let Some(cache) = path_cache {
            cache.clear_repo(repo_id);
        }

        Ok(root_fs_id)
    }

    /// Create a commit with the given root_fs_id and update the repo's HEAD.
    pub(crate) async fn create_commit(
        db: &DatabaseConnection,
        repo_id: &str,
        root_fs_id: &str,
        creator_name: &str,
        description: &str,
        path_cache: Option<&PathCache>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let now = chrono::Utc::now().timestamp();

        let repo_model = repo::Entity::find_by_id(repo_id).one(db).await?;
        let parent_commit_id = repo_model.as_ref().and_then(|r| r.head_commit_id.clone());

        let commit_data = crate::serialization::commit_json::CommitData {
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
        let commit_id = commit_data.compute_commit_id();

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
        commit::Entity::insert(commit_model).exec(db).await?;

        let mut repo_active: repo::ActiveModel = repo_model.unwrap().into();
        let commit_id_clone = commit_id.clone();
        repo_active.head_commit_id = sea_orm::Set(Some(commit_id));
        repo_active.updated_at = sea_orm::Set(now);
        repo_active.update(db).await?;

        if let Some(cache) = path_cache {
            cache.clear_repo(repo_id);
        }

        // Fire repo-update notification through the global broadcast channel.
        events_channel::publish_repo_update(repo_id, commit_id_clone);

        Ok(())
    }

    pub async fn read_dir_fs_object(
        db: &DatabaseConnection,
        repo_id: &str,
        fs_id: &str,
    ) -> Result<FsDirData, Box<dyn std::error::Error>> {
        crate::storage::read_fs_dir_data(db, repo_id, fs_id).await
    }

    pub async fn read_file_fs_object(
        db: &DatabaseConnection,
        repo_id: &str,
        fs_id: &str,
    ) -> Result<FsFileData, Box<dyn std::error::Error>> {
        crate::storage::read_fs_file_data(db, repo_id, fs_id).await
    }
}
