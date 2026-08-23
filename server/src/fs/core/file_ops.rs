use crate::domain;
use crate::repository::Repositories;
use base::common::{DirEntryData, EMPTY_SHA1, FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use futures::StreamExt;
use infra::crypto::random_key::encrypt_block;
use infra::entity::{commit, repo};
use infra::events;
use infra::storage::DynBlockStorage;
use sea_orm::DatabaseConnection;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use tokio::sync::Mutex;

/// Per-repo mutexes to prevent concurrent read-modify-write races
/// on FS tree directories. Without this, two concurrent requests
/// modifying the same directory can lose entries (one overwrites
/// the other's changes before a commit is created).
static REPO_WRITE_LOCKS: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Acquire the per-repo write lock. Blocks until no other FS tree
/// mutation is in progress for the given repo.
pub(crate) async fn acquire_repo_lock(repo_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let mut map = REPO_WRITE_LOCKS.lock().await;
    let lock = map
        .entry(repo_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())));
    let guard = lock.clone().lock_owned().await;
    drop(map); // release the hashmap lock before the long-running operation
    guard
}

/// Sentinel value indicating that no ancestor chain was pre-computed.
/// Callers that don't have an ancestor chain pass this to
/// `update_dir_tree_and_commit` / `update_dir_tree_no_commit`;
/// the walk_up_ancestors function will fall back to on-demand
/// path resolution for each ancestor level.
pub(crate) const EMPTY_ANCESTOR_CHAIN: &[(String, String)] = &[];

pub struct FileOps;

impl FileOps {
    /// Stream bytes from `stream` through the CDC chunker and write each
    /// resulting block to the content-addressed block store, returning the
    /// `block_ids` (in chunk order) and the aggregate `total_size` — the
    /// block-sequence twin of `create_file`'s in-memory chunking, so a
    /// resumable upload never has to hold the whole file in memory.
    ///
    /// When `enc_key` is `Some((key, iv))`, each block is encrypted with a
    /// deterministic IV before writing (matching `create_file`), yielding
    /// `block_id == sha1(encrypted_block)` so Seafile sync clients can still
    /// re-derive the content-addressed ids for encrypted repos.
    /// Feed `(idx, block)` pairs from `producer` to a bounded concurrent writer
    /// that persists them to the block store, so the producer's read loop never
    /// blocks on disk I/O. `buffered(4)` keeps block ids in input order (like
    /// `create_file`), and the bounded channel back-pressures the producer when
    /// the disk is the bottleneck. Returns `block_ids` in order plus the
    /// producer's `total_size`.
    pub(crate) async fn stream_blocks_pipelined<F, Fut>(
        store: &DynBlockStorage,
        enc_key: Option<(&[u8], &[u8])>,
        producer: F,
    ) -> Result<(Vec<String>, i64), AppError>
    where
        F: FnOnce(tokio::sync::mpsc::Sender<(usize, Vec<u8>)>) -> Fut,
        Fut: futures::Future<Output = Result<i64, AppError>>,
    {
        const CONCURRENCY: usize = 4;

        // The writer task needs owned handles and key material.
        let store = store.clone();
        let enc_key_owned: Option<(Vec<u8>, Vec<u8>)> =
            enc_key.map(|(key, iv)| (key.to_vec(), iv.to_vec()));

        let (tx, rx) = tokio::sync::mpsc::channel::<(usize, Vec<u8>)>(CONCURRENCY * 2);

        let writer = tokio::spawn(async move {
            futures::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|item| (item, rx))
            })
            .map(|(idx, blk)| {
                let store = store.clone();
                let enc_key = enc_key_owned.clone();
                async move {
                    let block_id = match &enc_key {
                        Some((key, iv)) => {
                            let encrypted = encrypt_block(&blk, key, iv);
                            store.write_block(&encrypted).await?
                        }
                        None => store.write_block(&blk).await?,
                    };
                    Ok((idx, block_id))
                }
            })
            .buffered(CONCURRENCY)
            .collect::<Vec<_>>()
            .await
        });

        let total_size = producer(tx).await?;

        let results: Vec<Result<(usize, String), AppError>> = writer
            .await
            .map_err(|e| AppError::Internal(format!("block writer join failed: {e}")))?;

        let mut block_ids = Vec::with_capacity(results.len());
        for r in results {
            let (idx, block_id) = r?;
            debug_assert_eq!(idx, block_ids.len(), "buffered preserves input order");
            block_ids.push(block_id);
        }

        Ok((block_ids, total_size))
    }

    pub async fn write_stream_blocks<S>(
        store: &DynBlockStorage,
        file_size: usize,
        stream: S,
        enc_key: Option<(&[u8], &[u8])>,
    ) -> Result<(Vec<String>, i64), AppError>
    where
        S: futures::Stream<Item = std::io::Result<bytes::Bytes>> + Unpin,
    {
        let mut stream = stream;
        Self::stream_blocks_pipelined(store, enc_key, move |tx| async move {
            let mut chunker = infra::storage::cdc::Chunker::new(file_size);
            let mut total_size: i64 = 0;
            let mut idx = 0usize;
            while let Some(chunk) = stream.next().await {
                let chunk =
                    chunk.map_err(|e| AppError::Internal(format!("stream read failed: {e}")))?;
                for blk in chunker.feed(chunk.as_ref()) {
                    total_size += blk.len() as i64;
                    tx.send((idx, blk))
                        .await
                        .map_err(|_| AppError::Internal("block writer stopped".into()))?;
                    idx += 1;
                }
            }
            let tail = chunker.finish();
            if !tail.is_empty() {
                total_size += tail.len() as i64;
                tx.send((idx, tail))
                    .await
                    .map_err(|_| AppError::Internal("block writer stopped".into()))?;
            }
            Ok(total_size)
        })
        .await
    }

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

        // Write blocks and store the fs object WITHOUT the per-repo lock:
        // blocks are content-addressed (SHA-1) and the fs_object insert is
        // INSERT OR IGNORE, so concurrent writers can't race here. Only the
        // read-modify-write of the FS tree below needs the lock — keeping the
        // (possibly slow) block I/O out of it lets concurrent writes to the
        // same repo proceed.
        let file_chunks = infra::storage::cdc::file_chunk_cdc(data);

        // Write blocks concurrently (encryption is CPU-bound, block I/O is
        // disk-bound — overlapping them raises throughput for large files).
        // `.buffered(4)` preserves input order, so block_ids stay in chunk
        // order. Blocks are content-addressed and this runs outside the
        // per-repo write lock, so concurrent writers can't race here.
        let results: Vec<Result<(usize, String), AppError>> =
            futures::stream::iter(file_chunks.iter().cloned().enumerate())
                .map(|(idx, (offset, size))| {
                    let chunk_data = &data[offset..offset + size];
                    let store = block_store.clone();
                    async move {
                        // Seafile encrypts each block with a deterministic IV
                        // derived from the key chain (enc_iv), NOT a per-block
                        // random IV — matches seafile-server seafile-crypt.c
                        // seafile_derive_key. The plaintext branch passes the
                        // original slice through, avoiding a whole-block copy.
                        let block_id = match enc_key {
                            Some((key, iv)) => {
                                let encrypted = encrypt_block(chunk_data, key, iv);
                                store.write_block(&encrypted).await?
                            }
                            None => store.write_block(chunk_data).await?,
                        };
                        Ok((idx, block_id))
                    }
                })
                .buffered(4)
                .collect::<Vec<_>>()
                .await;

        let mut block_ids = Vec::with_capacity(file_chunks.len());
        for r in results {
            let (idx, block_id) = r?;
            debug_assert_eq!(idx, block_ids.len(), "buffered preserves input order");
            block_ids.push(block_id);
        }
        let total_size: i64 = file_chunks.iter().map(|(_, size)| *size as i64).sum();

        let file_fs_data = FsFileData {
            block_ids,
            size: total_size,
            obj_type: 1,
            version: 1,
        };
        let file_fs_id = crate::fs::core::store_fs_file_object(db, repo_id, &file_fs_data).await?;

        // Serialize the FS tree mutation (read-modify-write + commit).
        let _lock = acquire_repo_lock(repo_id).await;

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

        // When replacing an existing file, keep the old entry untouched if the
        // content is identical (same file fs_id and size). That leaves the
        // directory hash (and thus the root) unchanged, so `create_commit`
        // dedups and no redundant commit is created. Without this, a
        // same-content re-upload would overwrite `mtime` and produce a
        // spurious new commit whenever the two uploads land in different
        // seconds.
        let mut identical_replace = false;
        if replace {
            identical_replace = dirents
                .iter()
                .any(|d| d.name == name && d.id == file_fs_id && d.size == total_size);
            if !identical_replace {
                dirents.retain(|d| d.name != name);
            }
        }

        if !identical_replace {
            dirents.push(DirEntryData {
                id: file_fs_id.clone(),
                mode: infra::serialization::S_IFREG,
                modifier: modifier.to_string(),
                mtime: now,
                name: name.to_string(),
                size: total_size,
            });
        }

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

        // Identical re-upload leaves the FS tree unchanged: the head commit
        // already reflects this state. Skip creating a redundant commit
        // (matches official seafile and avoids a commit_id collision when a
        // duplicate write lands in the same second).
        if let Some(head_id) = parent_commit_id.as_deref() {
            let head_commit = repos
                .commit
                .find_by_repo_and_commit_id(repo_id, head_id)
                .await?;
            if head_commit.is_some_and(|c| c.root_id == root_fs_id) {
                return Ok(file_fs_id);
            }
        }

        let commit_data = base::common::CommitData {
            commit_id: String::new(),
            repo_id: repo_id.to_string(),
            root_id: root_fs_id.clone(),
            creator_name: modifier.to_string(),
            creator: EMPTY_SHA1.to_string(),
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
            creator: sea_orm::Set(EMPTY_SHA1.to_string()),
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

    /// Commit a file whose blocks have already been written to the block
    /// store into the repo: create the fs_object, then perform the read-modify-
    /// write of the FS tree and the commit.
    ///
    /// This is the tree-submission half of [`FileOps::create_file`], split out
    /// so a streaming uploader can write blocks direct from the wire and then
    /// commit the resulting `block_ids` without ever holding the whole file in
    /// memory.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_file_from_blocks(
        db: &DatabaseConnection,
        repos: &Repositories,
        repo_id: &str,
        parent_path: &str,
        name: &str,
        block_ids: Vec<String>,
        total_size: i64,
        modifier: &str,
        replace: bool,
    ) -> Result<String, AppError> {
        let now = chrono::Utc::now().timestamp();

        let file_fs_data = FsFileData {
            block_ids,
            size: total_size,
            obj_type: 1,
            version: 1,
        };
        let file_fs_id = crate::fs::core::store_fs_file_object(db, repo_id, &file_fs_data).await?;

        // Serialize the FS tree mutation (read-modify-write + commit).
        let _lock = acquire_repo_lock(repo_id).await;

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

        // When replacing an existing file, keep the old entry untouched if the
        // content is identical (same file fs_id and size). That leaves the
        // directory hash (and thus the root) unchanged, so `create_commit`
        // dedups and no redundant commit is created. Without this, a
        // same-content re-upload would overwrite `mtime` and produce a
        // spurious new commit whenever the two uploads land in different
        // seconds.
        let mut identical_replace = false;
        if replace {
            identical_replace = dirents
                .iter()
                .any(|d| d.name == name && d.id == file_fs_id && d.size == total_size);
            if !identical_replace {
                dirents.retain(|d| d.name != name);
            }
        }

        if !identical_replace {
            dirents.push(DirEntryData {
                id: file_fs_id.clone(),
                mode: infra::serialization::S_IFREG,
                modifier: modifier.to_string(),
                mtime: now,
                name: name.to_string(),
                size: total_size,
            });
        }

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

        // Identical re-upload leaves the FS tree unchanged: the head commit
        // already reflects this state. Skip creating a redundant commit
        // (matches official seafile and avoids a commit_id collision when a
        // duplicate write lands in the same second).
        if let Some(head_id) = parent_commit_id.as_deref() {
            let head_commit = repos
                .commit
                .find_by_repo_and_commit_id(repo_id, head_id)
                .await?;
            if head_commit.is_some_and(|c| c.root_id == root_fs_id) {
                return Ok(file_fs_id);
            }
        }

        let commit_data = base::common::CommitData {
            commit_id: String::new(),
            repo_id: repo_id.to_string(),
            root_id: root_fs_id.clone(),
            creator_name: modifier.to_string(),
            creator: EMPTY_SHA1.to_string(),
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
            creator: sea_orm::Set(EMPTY_SHA1.to_string()),
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
    /// Delegates to `tree::resolve_fs_id` after resolving root fs_id.
    async fn resolve_fs_id(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
    ) -> Result<String, AppError> {
        if path == "/" {
            return Self::resolve_root_fs_id(repos, repo_id).await;
        }
        let root_fs_id = Self::resolve_root_fs_id(repos, repo_id).await?;
        crate::fs::core::resolve_fs_id(repos, repo_id, &root_fs_id, path).await
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
        let root_fs_id = Self::apply_tree_update(
            db,
            repos,
            repo_id,
            parent_path,
            parent_fs_id,
            ancestor_chain,
            update_fn,
        )
        .await?;

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
        Self::apply_tree_update(
            db,
            repos,
            repo_id,
            parent_path,
            parent_fs_id,
            ancestor_chain,
            update_fn,
        )
        .await
    }

    /// Shared body of the two `update_dir_tree_*` helpers: lock the repo,
    /// apply `update_fn` to the parent directory, persist it and walk up the
    /// ancestor chain to recompute the root fs_id.
    async fn apply_tree_update(
        db: &DatabaseConnection,
        repos: &Repositories,
        repo_id: &str,
        parent_path: &str,
        parent_fs_id: &str,
        ancestor_chain: &[(String, String)],
        update_fn: impl FnOnce(&mut Vec<DirEntryData>) -> Result<(), AppError>,
    ) -> Result<String, AppError> {
        let _lock = acquire_repo_lock(repo_id).await;
        let mut parent_data = Self::read_dir_fs_object(repos, repo_id, parent_fs_id).await?;
        update_fn(&mut parent_data.dirents)?;

        let new_parent_fs_id =
            crate::fs::core::store_fs_dir_object(db, repo_id, &parent_data).await?;

        if parent_path == "/" {
            Ok(new_parent_fs_id)
        } else {
            Self::walk_up_ancestors(
                repos,
                db,
                repo_id,
                parent_path,
                &new_parent_fs_id,
                ancestor_chain,
            )
            .await
        }
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

        // Skip a commit when the tree didn't actually change. Defensive: a
        // no-op delete/move/rename would otherwise produce a commit_id
        // collision (unique (repo_id, commit_id)) when issued in the same
        // second with an unchanged root.
        if let Some(head_id) = parent_commit_id.as_deref() {
            let head_commit = repos
                .commit
                .find_by_repo_and_commit_id(repo_id, head_id)
                .await?;
            if head_commit.is_some_and(|c| c.root_id == root_fs_id) {
                return Ok(());
            }
        }

        let commit_data = base::common::CommitData {
            commit_id: String::new(),
            repo_id: repo_id.to_string(),
            root_id: root_fs_id.to_string(),
            creator_name: creator_name.to_string(),
            creator: EMPTY_SHA1.to_string(),
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
            creator: sea_orm::Set(EMPTY_SHA1.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, DynBlockStorage) {
        let dir = tempfile::tempdir().unwrap();
        let store = infra::storage::new_block_store(dir.path());
        (dir, store)
    }

    /// A deterministic pseudo-random byte vector (no rand dependency).
    fn pseudo_data(len: usize) -> Vec<u8> {
        let mut x: u64 = 0x9E3779B97F4A7C15;
        (0..len)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (x >> 33) as u8
            })
            .collect()
    }

    fn bytes_stream(
        data: Vec<u8>,
        chunk_size: usize,
    ) -> impl futures::Stream<Item = std::io::Result<bytes::Bytes>> {
        let items: Vec<std::io::Result<bytes::Bytes>> = data
            .chunks(chunk_size)
            .map(|c| Ok(bytes::Bytes::copy_from_slice(c)))
            .collect();
        futures::stream::iter(items)
    }

    /// The streaming path must emit exactly the block ids that the whole-buffer
    /// `create_file` chunking produces, in order, across more than four blocks
    /// (so the pipeline concurrency and ordering are both exercised).
    #[tokio::test]
    async fn test_write_stream_blocks_matches_create_file_chunking() {
        let (_dir, store) = temp_store();
        let data = pseudo_data(6 * 1024 * 1024 + 123); // >4 blocks at ~1MiB avg

        // Reference: create_file's whole-buffer chunking.
        let chunks = infra::storage::cdc::file_chunk_cdc(&data);
        assert!(chunks.len() > 1, "fixture must span multiple blocks");
        let mut expected_ids = Vec::new();
        for (offset, size) in &chunks {
            let id = store
                .write_block(&data[*offset..offset + size])
                .await
                .unwrap();
            expected_ids.push(id);
        }

        let stream = bytes_stream(data.clone(), 8192);
        let (ids, total) = FileOps::write_stream_blocks(&store, data.len(), stream, None)
            .await
            .unwrap();
        assert_eq!(ids, expected_ids);
        assert_eq!(total as usize, data.len());
    }

    /// Directly exercise the pipeline with more blocks than the channel
    /// capacity plus the in-flight window (8 + 4), so full-load ordering and
    /// content addressing are both verified.
    #[tokio::test]
    async fn test_stream_blocks_pipelined_many_blocks_in_order() {
        let (_dir, store) = temp_store();
        let (ids, total) = FileOps::stream_blocks_pipelined(&store, None, move |tx| async move {
            for i in 0..12u8 {
                tx.send((i as usize, vec![i; 128]))
                    .await
                    .map_err(|_| AppError::Internal("block writer stopped".into()))?;
            }
            Ok(12i64 * 128)
        })
        .await
        .unwrap();
        assert_eq!(ids.len(), 12);
        assert_eq!(total, 12 * 128);
        // Content addressing: each block id round-trips to its bytes in order.
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(store.read_block(id).await.unwrap(), vec![i as u8; 128]);
        }
    }

    /// A stream that errors mid-way must surface as a request failure.
    #[tokio::test]
    async fn test_write_stream_blocks_error_propagation() {
        let (_dir, store) = temp_store();
        let stream = futures::stream::iter(vec![
            Ok(bytes::Bytes::from_static(b"hello")),
            Err(std::io::Error::other("boom")),
        ]);
        let result = FileOps::write_stream_blocks(&store, 10, stream, None).await;
        assert!(result.is_err(), "stream error must propagate");
    }
}
