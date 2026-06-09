pub mod block_store;
pub mod cdc;
pub mod download;
pub mod file_ops;
pub mod gc;
pub mod path_cache;
pub mod versioning;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::entity::fs_object;
use crate::error::AppError;
use crate::serialization::fs_json::{FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE};
use crate::storage::path_cache::PathCache;

/// Abstract backend for content-addressed block storage.
///
/// Blocks are identified by their SHA-1 hash (40-char hex string) and stored
/// in a two-level directory tree: `{base}/{prefix[..2]}/{block_id}`.
#[async_trait::async_trait]
pub trait BlockStorageBackend: Send + Sync {
    /// Check if a block exists on disk.
    async fn has_block(&self, block_id: &str) -> bool;

    /// Read raw block data by its SHA-1 ID.
    async fn read_block(&self, block_id: &str) -> Result<Vec<u8>, std::io::Error>;

    /// Write raw block data, computing and returning its SHA-1 ID.
    async fn write_block(&self, data: &[u8]) -> Result<String, std::io::Error>;

    /// Delete a block file from disk.
    async fn remove_block(&self, block_id: &str) -> Result<(), std::io::Error>;

    /// List all block IDs stored on disk.
    async fn list_blocks(&self) -> Result<Vec<String>, std::io::Error>;
}

/// Convenience alias for an Arc-wrapped block storage backend.
pub type DynBlockStorage = Arc<dyn BlockStorageBackend>;

/// Create a new filesystem-backed block store at the given directory.
pub fn new_block_store(base_dir: &std::path::Path) -> DynBlockStorage {
    Arc::new(block_store::BlockStorage::new(base_dir.to_path_buf()))
}

// ====================================================================
// Unified FS tree access functions
// ====================================================================

/// Serialize an FsFileData to JSON, compute its SHA1 ID, check the DB,
/// and insert if the object does not already exist. Returns the fs_id.
///
/// This matches seafile's write_seafile() flow:
///   seafile_to_json() → calculate SHA1 → check exists → write.
pub async fn store_fs_file_object(
    db: &DatabaseConnection,
    repo_id: &str,
    file_data: FsFileData,
) -> Result<String, AppError> {
    let json = file_data.to_compact_json();
    let fs_id = crate::crypto::fs_id::sha1_hex(json.as_bytes());

    let exists = fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(repo_id))
        .filter(fs_object::Column::FsId.eq(&fs_id))
        .one(db)
        .await?
        .is_some();

    if !exists {
        fs_object::Entity::insert(fs_object::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: sea_orm::Set(repo_id.to_string()),
            fs_id: sea_orm::Set(fs_id.clone()),
            obj_type: sea_orm::Set(SEAF_METADATA_TYPE_FILE as i8),
            data: sea_orm::Set(json),
        })
        .exec(db)
        .await?;
    }

    Ok(fs_id)
}

/// Serialize an FsDirData to JSON, compute its SHA1 ID, check the DB,
/// and insert if the object does not already exist. Returns the fs_id.
///
/// Empty directories use the EMPTY_SHA1 sentinel and are never stored,
/// matching seafile's seaf_dir_save() / seaf_dir_new() behavior:
///   seaf_dir_new: entries==NULL → dir_id = EMPTY_SHA1 (line 1455)
///   seaf_dir_save: dir_id == EMPTY_SHA1 → skip (line 1861)
pub async fn store_fs_dir_object(
    db: &DatabaseConnection,
    repo_id: &str,
    dir_data: FsDirData,
) -> Result<String, AppError> {
    // Empty dirs use the EMPTY_SHA1 sentinel per seafile convention.
    // They are never persisted — the all-zero ID is recognized by both
    // server and client as "empty directory". Storing a computed hash
    // would break client sync (the client would not create the folder).
    if dir_data.dirents.is_empty() {
        return Ok("0000000000000000000000000000000000000000".to_string());
    }

    let json = dir_data.to_compact_json();
    let fs_id = crate::crypto::fs_id::sha1_hex(json.as_bytes());

    let exists = fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(repo_id))
        .filter(fs_object::Column::FsId.eq(&fs_id))
        .one(db)
        .await?
        .is_some();

    if !exists {
        fs_object::Entity::insert(fs_object::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: sea_orm::Set(repo_id.to_string()),
            fs_id: sea_orm::Set(fs_id.clone()),
            obj_type: sea_orm::Set(SEAF_METADATA_TYPE_DIR as i8),
            data: sea_orm::Set(json),
        })
        .exec(db)
        .await?;
    }

    Ok(fs_id)
}

/// Read and parse a directory fs_object (FsDirData) from the database.
pub async fn read_fs_dir_data(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsDirData, Box<dyn std::error::Error>> {
    // The zero hash (all zeros) is a sentinel in seafile's protocol,
    // used for empty/incomplete directories or when an fs_object
    // hasn't been fully committed yet. Treat it as an empty directory.
    if fs_id == "0000000000000000000000000000000000000000" {
        return Ok(FsDirData {
            dirents: vec![],
            obj_type: crate::serialization::fs_json::SEAF_METADATA_TYPE_DIR,
            version: 1,
        });
    }
    let obj = fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(repo_id))
        .filter(fs_object::Column::FsId.eq(fs_id))
        .one(db)
        .await?
        .ok_or_else(|| format!("fs_object not found: {fs_id}"))?;
    let data: FsDirData = serde_json::from_str(&obj.data)?;
    Ok(data)
}

/// Read and parse a file fs_object (FsFileData) from the database.
pub async fn read_fs_file_data(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsFileData, Box<dyn std::error::Error>> {
    let obj = fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(repo_id))
        .filter(fs_object::Column::FsId.eq(fs_id))
        .one(db)
        .await?
        .ok_or_else(|| format!("fs_object not found: {fs_id}"))?;
    let data: FsFileData = serde_json::from_str(&obj.data)?;
    Ok(data)
}

/// Traverse the FS tree from root_fs_id following path segments,
/// returning the fs_id of the final segment.
///
/// Path should be absolute (e.g. `/dir/subdir/file.txt`).
/// Returns the root_fs_id itself if path is "/" or empty.
///
/// When `cache` is provided, intermediate directories are cached so
/// subsequent traversals of sibling paths skip earlier SQL queries.
pub async fn resolve_fs_id(
    db: &DatabaseConnection,
    repo_id: &str,
    root_fs_id: &str,
    path: &str,
    cache: Option<&PathCache>,
) -> Result<String, Box<dyn std::error::Error>> {
    let segments: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Ok(root_fs_id.to_string());
    }

    // Check cache for the full path first.
    if let Some(cache) = cache
        && let Some((_, fs_id, _)) = cache.get(repo_id, root_fs_id, path)
    {
        return Ok(fs_id);
    }

    let mut current_fs_id = root_fs_id.to_string();
    let mut accumulated = String::new();

    for (i, segment) in segments.iter().enumerate() {
        let dir_data = read_fs_dir_data(db, repo_id, &current_fs_id).await?;

        // Build the path for this segment.
        if i == 0 {
            accumulated = format!("/{}", segment);
        } else {
            accumulated = format!("{}/{}", accumulated, segment);
        }

        // Find the entry FIRST and update current_fs_id to the child's fs_id,
        // then cache using the CHILD's (correct) fs_id and data.
        let entry = dir_data
            .dirents
            .iter()
            .find(|d| d.name == *segment)
            .ok_or_else(|| format!("path segment not found: {segment}"))?;

        if i + 1 < segments.len()
            && let Some(cache) = cache
        {
            // Read the child directory's FsDirData so the cache stores the
            // correct content, NOT the parent's data.
            let child_fs_id = entry.id.clone();
            match read_fs_dir_data(db, repo_id, &child_fs_id).await {
                Ok(child_data) => {
                    let child_json = child_data.to_compact_json();
                    cache.set_dir(repo_id, root_fs_id, &accumulated, &child_fs_id, &child_json);
                }
                Err(_) => {
                    // Child may be EMPTY_SHA1 sentinel or a file — cache
                    // a minimal empty dir so callers can still find it.
                    cache.set_dir(
                        repo_id,
                        root_fs_id,
                        &accumulated,
                        &child_fs_id,
                        r#"{"dirents":[],"type":3,"version":1}"#,
                    );
                }
            }
        } else {
            // Even for the final segment, cache the resolved fs_id so
            // subsequent resolve_fs_id calls for the same path hit quickly.
            if let Some(cache) = cache {
                let child_fs_id = entry.id.clone();
                cache.set_dir(
                    repo_id,
                    root_fs_id,
                    &accumulated,
                    &child_fs_id,
                    r#"{"dirents":[],"type":3,"version":1}"#,
                );
            }
        }

        current_fs_id = entry.id.clone();
    }

    // Cache the final resolution for the full path (already cached
    // segment-by-segment above for intermediate paths, but the full
    // path may differ on the final segment).
    if let Some(cache) = cache {
        let fake_json = r#"{"dirents":[],"type":3,"version":1}"#.to_string();
        cache.set_dir(repo_id, root_fs_id, path, &current_fs_id, &fake_json);
    }

    Ok(current_fs_id)
}

/// Read and parse a directory fs_object, consulting the path cache first.
///
/// Prefer this over `read_fs_dir_data` when you have the `path` and
/// `root_fs_id` available — it avoids the SQL query on a cache hit.
pub async fn read_fs_dir_data_cached(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
    cache: &PathCache,
    root_fs_id: &str,
    path: &str,
) -> Result<FsDirData, Box<dyn std::error::Error>> {
    // Check cache first.
    if let Some((3, cached_fs_id, json)) = cache.get(repo_id, root_fs_id, path)
        && cached_fs_id == fs_id
    {
        let data: FsDirData = serde_json::from_str(&json)?;
        return Ok(data);
    }

    // Fall back to DB.
    let data = read_fs_dir_data(db, repo_id, fs_id).await?;
    let json = data.to_compact_json();
    cache.set_dir(repo_id, root_fs_id, path, fs_id, &json);
    Ok(data)
}

/// Read and parse a file fs_object, consulting the path cache first.
pub async fn read_fs_file_data_cached(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
    cache: &PathCache,
    root_fs_id: &str,
    path: &str,
) -> Result<FsFileData, Box<dyn std::error::Error>> {
    // Check cache first.
    if let Some((1, cached_fs_id, json)) = cache.get(repo_id, root_fs_id, path)
        && cached_fs_id == fs_id
    {
        let data: FsFileData = serde_json::from_str(&json)?;
        return Ok(data);
    }

    // Fall back to DB.
    let data = read_fs_file_data(db, repo_id, fs_id).await?;
    let json = crate::serialization::fs_json::FsFileData::to_compact_json(&data);
    cache.set_file(repo_id, root_fs_id, path, fs_id, &json);
    Ok(data)
}
