use crate::domain;
use crate::repository::Repositories;
use base::common::{DirEntryData, EMPTY_SHA1, FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use futures::StreamExt;
use infra::crypto::fs_id::sha1_hex;
use infra::crypto::random_key::encrypt_block;
use infra::entity::commit;
use infra::events;
use infra::storage::DynBlockStorage;
use sea_orm::DatabaseConnection;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use tokio::sync::Mutex;

/// Per-repo mutexes serializing FS tree read-modify-write. Without them two
/// concurrent requests on one directory can lose entries before a commit lands.
static REPO_WRITE_LOCKS: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Acquire the per-repo write lock, blocking until no other FS tree mutation is
/// in progress for that repo.
pub(crate) async fn acquire_repo_lock(repo_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let mut map = REPO_WRITE_LOCKS.lock().await;
    let lock = map
        .entry(repo_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())));
    let guard = lock.clone().lock_owned().await;
    drop(map); // release the hashmap lock before the long-running operation
    guard
}

/// Sentinel for "no pre-computed ancestor chain": `walk_up_ancestors` then
/// resolves each ancestor level on demand.
pub(crate) const EMPTY_ANCESTOR_CHAIN: &[(String, String)] = &[];

pub struct FileOps;

impl FileOps {
    /// Stream bytes through the CDC chunker, write each block to the
    /// content-addressed store, and return `(block_ids, total_size,
    /// new_block_ids)` — the blocks **newly written** by this call, so a caller
    /// whose quota check fails can clean up only its own blocks (deduped blocks
    /// are excluded) without holding the file in memory.
    ///
    /// `enc_key` encrypts each block with a deterministic IV (matching
    /// `create_file`), yielding `block_id == sha1(encrypted_block)` so Seafile
    /// sync clients can re-derive the ids for encrypted repos. `producer` feeds
    /// `(idx, chunk)` pairs to a bounded concurrent writer; `buffered(4)`
    /// preserves input order and the channel back-pressures the producer when
    /// the disk is the bottleneck.
    pub(crate) async fn stream_blocks_pipelined<F, Fut>(
        store: &DynBlockStorage,
        repo_id: &str,
        enc_key: Option<(&[u8], &[u8])>,
        producer: F,
    ) -> Result<(Vec<String>, i64, Vec<String>), AppError>
    where
        F: FnOnce(tokio::sync::mpsc::Sender<(usize, Vec<u8>)>) -> Fut,
        Fut: futures::Future<Output = Result<i64, AppError>>,
    {
        const CONCURRENCY: usize = 4;

        let store = store.clone();
        let repo_id = repo_id.to_string();
        let enc_key_owned: Option<(Vec<u8>, Vec<u8>)> =
            enc_key.map(|(key, iv)| (key.to_vec(), iv.to_vec()));

        let (tx, rx) = tokio::sync::mpsc::channel::<(usize, Vec<u8>)>(CONCURRENCY * 2);

        let writer = tokio::spawn(async move {
            futures::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|item| (item, rx))
            })
            .map(|(idx, blk)| {
                let store = store.clone();
                let repo_id = repo_id.clone();
                let enc_key = enc_key_owned.clone();
                async move {
                    let (block_id, was_new) = match &enc_key {
                        Some((key, iv)) => {
                            // AES-CBC is CPU-bound: keep it off the async worker.
                            let key = key.clone();
                            let iv = iv.clone();
                            let blk = blk.clone();
                            let encrypted =
                                tokio::task::spawn_blocking(move || encrypt_block(&blk, &key, &iv))
                                    .await
                                    .map_err(|e| AppError::internal(e.to_string()))?;
                            let id = sha1_hex(&encrypted);
                            store
                                .write_block_with_id_tracked(&repo_id, &id, &encrypted)
                                .await?
                        }
                        None => {
                            let id = sha1_hex(&blk);
                            store
                                .write_block_with_id_tracked(&repo_id, &id, &blk)
                                .await?
                        }
                    };
                    Ok((idx, block_id, was_new))
                }
            })
            .buffered(CONCURRENCY)
            .collect::<Vec<_>>()
            .await
        });

        let total_size = producer(tx).await?;

        let results: Vec<Result<(usize, String, bool), AppError>> = writer
            .await
            .map_err(|e| AppError::Internal(format!("block writer join failed: {e}")))?;

        let mut block_ids = Vec::with_capacity(results.len());
        let mut new_block_ids = Vec::with_capacity(results.len());
        for r in results {
            let (idx, block_id, was_new) = r?;
            debug_assert_eq!(idx, block_ids.len(), "buffered preserves input order");
            if was_new {
                new_block_ids.push(block_id.clone());
            }
            block_ids.push(block_id);
        }

        Ok((block_ids, total_size, new_block_ids))
    }

    pub async fn write_stream_blocks<S>(
        store: &DynBlockStorage,
        repo_id: &str,
        file_size: usize,
        stream: S,
        enc_key: Option<(&[u8], &[u8])>,
    ) -> Result<(Vec<String>, i64, Vec<String>), AppError>
    where
        S: futures::Stream<Item = std::io::Result<bytes::Bytes>> + Unpin,
    {
        let mut stream = stream;
        Self::stream_blocks_pipelined(store, repo_id, enc_key, move |tx| async move {
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

    /// Commit a file whose blocks are already in the block store: create the
    /// fs_object, then read-modify-write the FS tree and the commit. The
    /// streaming uploader writes blocks straight from the wire and never holds
    /// the whole file in memory.
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
    ) -> Result<(String, String), AppError> {
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

        // Resolve the parent, pre-computing the ancestor chain to avoid O(d²) re-resolution.
        let (parent_fs_id, ancestor_chain) = if parent_path == "/" {
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

        // An identical replace (same fs_id and size) leaves the old entry
        // untouched, so the directory hash — and thus the commit id — is
        // unchanged and no spurious commit appears when the re-upload lands in a
        // different second.
        let mut identical_replace = false;
        if replace {
            identical_replace = dirents
                .iter()
                .any(|d| d.name == name && d.id == file_fs_id && d.size == total_size);
            if !identical_replace {
                dirents.retain(|d| d.name != name);
            }
        }

        // Name this upload is stored under. `replace=0` is the official default
        // (Android/iOS "keep both", the desktop uploader, a plain POST) and must
        // keep the existing entry, storing the new file under a unique name like
        // seafile's `genUniqueName()` (`server/fileserver/fileop.go:2407-2434`);
        // overwriting in place is reserved for `replace=1` / the update endpoints.
        let effective_name = if identical_replace {
            name.to_string()
        } else if !replace && dirents.iter().any(|d| d.name == name) {
            infra::common::util::generate_unique_filename(&dirents, name)
        } else {
            name.to_string()
        };

        if !identical_replace {
            dirents.push(DirEntryData {
                id: file_fs_id.clone(),
                mode: infra::serialization::S_IFREG,
                modifier: modifier.to_string(),
                mtime: now,
                name: effective_name.clone(),
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

        // Identical re-upload leaves the FS tree unchanged, so skip the redundant
        // commit (also avoids a commit_id collision within the same second).
        if let Some(head_id) = parent_commit_id.as_deref() {
            let head_commit = repos
                .commit
                .find_by_repo_and_commit_id(repo_id, head_id)
                .await?;
            if head_commit.is_some_and(|c| c.root_id == root_fs_id) {
                return Ok((file_fs_id, effective_name));
            }
        }

        let commit_data = base::common::CommitData {
            commit_id: String::new(),
            repo_id: repo_id.to_string(),
            root_id: root_fs_id.clone(),
            creator_name: modifier.to_string(),
            creator: EMPTY_SHA1.to_string(),
            description: format!("Added {}", effective_name),
            ctime: now,
            parent_id: parent_commit_id.clone(),
            second_parent_id: None,
            repo_name: None,
            repo_desc: None,
            repo_category: None,
            encrypted: None,
            enc_version: None,
            magic: None,
            salt: None,
            pwd_hash: None,
            pwd_hash_algo: None,
            pwd_hash_params: None,
            key: None,
            version: 1,
            conflict: None,
            new_merge: None,
        };
        let commit_id = domain::commit::compute_commit_id(&commit_data);

        let commit_model = commit::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: sea_orm::Set(repo_id.to_string()),
            commit_id: sea_orm::Set(commit_id.clone()),
            root_id: sea_orm::Set(root_fs_id),
            parent_id: sea_orm::Set(parent_commit_id.clone()),
            second_parent_id: sea_orm::NotSet,
            creator_name: sea_orm::Set(modifier.to_string()),
            creator: sea_orm::Set(EMPTY_SHA1.to_string()),
            description: sea_orm::Set(format!("Added {}", name)),
            ctime: sea_orm::Set(now),
            version: sea_orm::Set(1i8),
            new_merge: sea_orm::NotSet,
            conflict: sea_orm::NotSet,
        };
        repos.commit.insert(commit_model).await?;

        if repo_model.is_none() {
            return Err(AppError::NotFound("repo not found".into()));
        }

        // Same compare-and-swap as `create_commit`: the per-repo lock serializes
        // REST writes only, so an upload may have advanced HEAD since it was read.
        if !repos
            .repo
            .test_and_update_head_commit(repo_id, parent_commit_id.as_deref(), &commit_id, now)
            .await?
        {
            return Err(AppError::Conflict(format!(
                "HEAD of {repo_id} advanced while this upload was being committed"
            )));
        }

        // Notify clients through the broadcast channel; otherwise the Seafile
        // client learns about the file only on its next poll.
        events::publish_repo_update(repo_id, commit_id);

        Ok((file_fs_id, effective_name))
    }

    /// Walk from the immediate parent up to root, rewriting each ancestor's
    /// `FsDirData` to reference the new child fs_id; returns the new root fs_id.
    /// `ancestor_chain` (root-down `(path, fs_id)` pairs) turns each level's
    /// `resolve_fs_id` into a lookup, reducing O(d²) to O(d).
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

        // Path→fs_id map for O(1) lookups; an empty chain falls through to
        // on-demand `resolve_fs_id` per level.
        let chain_map: std::collections::HashMap<&str, &str> = ancestor_chain
            .iter()
            .map(|(p, id)| (p.as_str(), id.as_str()))
            .collect();

        loop {
            let (parent_path, child_name) = match current_child_path.rsplit_once('/') {
                Some(("", name)) => ("/".to_string(), name.to_string()),
                Some((parent, name)) => (parent.to_string(), name.to_string()),
                None => {
                    return Ok(current_child_fs_id);
                }
            };

            // Use the chain when it has this level, else resolve from root.
            let ancestor_fs_id = if parent_path == "/" {
                Self::resolve_root_fs_id(repos, repo_id).await?
            } else {
                match chain_map.get(parent_path.as_str()) {
                    Some(id) => id.to_string(),
                    None => Self::resolve_fs_id(repos, repo_id, &parent_path).await?,
                }
            };

            let mut ancestor_data =
                Self::read_dir_fs_object(repos, repo_id, &ancestor_fs_id).await?;

            let mut found = false;
            for entry in &mut ancestor_data.dirents {
                if entry.name == child_name {
                    entry.id = current_child_fs_id.clone();
                    found = true;
                    break;
                }
            }

            if !found {
                // `create_dir` always records the child, so this is a defensive
                // fallback using S_IFDIR defaults.
                ancestor_data.dirents.push(DirEntryData {
                    id: current_child_fs_id.clone(),
                    mode: infra::serialization::S_IFDIR,
                    modifier: String::new(),
                    mtime: chrono::Utc::now().timestamp(),
                    name: child_name.clone(),
                    size: 0,
                });
            }

            let new_ancestor_fs_id =
                crate::fs::core::store_fs_dir_object(db, repo_id, &ancestor_data).await?;

            if parent_path == "/" {
                return Ok(new_ancestor_fs_id);
            }

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
        crate::fs::core::resolve_fs_id(repos, repo_id, &root_fs_id, path).await
    }

    /// Resolve a path to its fs_id, also returning the fs_id of every
    /// intermediate directory root-down, e.g. for `"/a/b/c"`:
    /// `[("/a", fs_a), ("/a/b", fs_b), ("/a/b/c", fs_c)]`. Feeding this to
    /// `walk_up_ancestors` reduces O(d²) to O(d).
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
            // Every component of a directory path must be a directory: a
            // zero-byte file's id is `EMPTY_SHA1`, which `read_fs_dir_data` reads
            // back as an empty directory, so a file name would otherwise resolve.
            if !crate::fs::core::tree::is_dir_mode(found.mode) {
                return Err(AppError::NotFound(format!(
                    "not a directory: '{}' in '{}'",
                    part, path
                )));
            }
            current_fs_id = found.id.clone();
            chain.push((accumulated.clone(), current_fs_id.clone()));
        }

        Ok((current_fs_id, chain))
    }

    /// Apply `update_fn` to the parent directory's entries, persist the tree up
    /// to the root, create a commit and update HEAD. `parent_fs_id` is the parent
    /// id **before** modification.
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

    /// Same as `update_dir_tree_and_commit` without the commit, so a multi-step
    /// operation (e.g. move) can update several parts of the tree and commit
    /// once. Returns the new root fs_id.
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

    /// Shared body of the two `update_dir_tree_*` helpers: lock the repo, apply
    /// `update_fn`, persist and walk up the ancestors to get the root fs_id.
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

        // Skip a commit when the tree didn't change: a no-op delete/move/rename
        // would otherwise collide on the unique (repo_id, commit_id) key.
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
            salt: None,
            pwd_hash: None,
            pwd_hash_algo: None,
            pwd_hash_params: None,
            key: None,
            version: 1,
            conflict: None,
            new_merge: None,
        };
        let commit_id = domain::commit::compute_commit_id(&commit_data);

        let commit_model = commit::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: sea_orm::Set(repo_id.to_string()),
            commit_id: sea_orm::Set(commit_id.clone()),
            root_id: sea_orm::Set(root_fs_id.to_string()),
            parent_id: sea_orm::Set(parent_commit_id.clone()),
            second_parent_id: sea_orm::NotSet,
            creator_name: sea_orm::Set(creator_name.to_string()),
            creator: sea_orm::Set(EMPTY_SHA1.to_string()),
            description: sea_orm::Set(description.to_string()),
            ctime: sea_orm::Set(now),
            version: sea_orm::Set(1i8),
            new_merge: sea_orm::NotSet,
            conflict: sea_orm::NotSet,
        };
        repos.commit.insert(commit_model).await?;

        if repo_model.is_none() {
            return Err(AppError::NotFound("repo not found".into()));
        }
        let commit_id_clone = commit_id.clone();

        // Move HEAD only while it is still the commit this tree was computed
        // from. A client upload or another REST request can advance HEAD between
        // the read above and this write; overwriting it would silently drop that
        // commit, so report the race instead — the tree was built from the old
        // HEAD, and the caller can re-read and retry.
        if !repos
            .repo
            .test_and_update_head_commit(
                repo_id,
                parent_commit_id.as_deref(),
                &commit_id_clone,
                now,
            )
            .await?
        {
            return Err(AppError::Conflict(format!(
                "HEAD of {repo_id} advanced while this change was being committed"
            )));
        }

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

    /// Block operations are repo-scoped; the pipeline itself is what is under test.
    const REPO: &str = "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d";

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

    /// The streaming path must emit exactly the ids the whole-buffer
    /// `create_file` chunking produces, in order, for a multi-block file.
    #[tokio::test]
    async fn test_write_stream_blocks_matches_create_file_chunking() {
        let (_dir, store) = temp_store();
        // 12 MiB spans two CDC blocks at seafile's 8/6/10 MiB defaults (the first
        // hits the 10 MiB max).
        let data = pseudo_data(12 * 1024 * 1024 + 123);

        // Reference: create_file's whole-buffer chunking.
        let chunks = infra::storage::cdc::file_chunk_cdc(&data);
        assert!(chunks.len() > 1, "fixture must span multiple blocks");
        let mut expected_ids = Vec::new();
        for (offset, size) in &chunks {
            let id = store
                .write_block(REPO, &data[*offset..offset + size])
                .await
                .unwrap();
            expected_ids.push(id);
        }

        let stream = bytes_stream(data.clone(), 8192);
        let (ids, total, _new_ids) =
            FileOps::write_stream_blocks(&store, REPO, data.len(), stream, None)
                .await
                .unwrap();
        assert_eq!(ids, expected_ids);
        assert_eq!(total as usize, data.len());
    }

    /// More blocks than the channel capacity plus the in-flight window (8 + 4),
    /// so full-load ordering and content addressing are both verified.
    #[tokio::test]
    async fn test_stream_blocks_pipelined_many_blocks_in_order() {
        let (_dir, store) = temp_store();
        let (ids, total, new_ids) =
            FileOps::stream_blocks_pipelined(&store, REPO, None, move |tx| async move {
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
        assert_eq!(new_ids.len(), 12);
        // Content addressing: each block id round-trips to its bytes in order.
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(
                store.read_block(REPO, id).await.unwrap(),
                vec![i as u8; 128]
            );
        }
    }

    #[tokio::test]
    async fn test_write_stream_blocks_error_propagation() {
        let (_dir, store) = temp_store();
        let stream = futures::stream::iter(vec![
            Ok(bytes::Bytes::from_static(b"hello")),
            Err(std::io::Error::other("boom")),
        ]);
        let result = FileOps::write_stream_blocks(&store, REPO, 10, stream, None).await;
        assert!(result.is_err(), "stream error must propagate");
    }
}
