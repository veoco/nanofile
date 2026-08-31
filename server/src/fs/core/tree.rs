use std::collections::HashMap;

use crate::repository::Repositories;
use base::common::{FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE};
use base::error::AppError;
use infra::common::EMPTY_SHA1;
use infra::entity::fs_object;

/// Read and parse a directory fs_object (FsDirData) from the database.
pub async fn read_fs_dir_data(
    repos: &Repositories,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsDirData, AppError> {
    // The zero hash (all zeros) is a sentinel in seafile's protocol,
    // used for empty/incomplete directories or when an fs_object
    // hasn't been fully committed yet. Treat it as an empty directory.
    if fs_id == EMPTY_SHA1 {
        return Ok(FsDirData {
            dirents: vec![],
            obj_type: SEAF_METADATA_TYPE_DIR,
            version: 1,
        });
    }
    let obj = repos
        .fs_object
        .find_by_repo_and_fs_id(repo_id, fs_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("fs_object not found: {fs_id}")))?;
    let data: FsDirData =
        serde_json::from_str(&obj.data).map_err(|e| AppError::internal(e.to_string()))?;
    Ok(data)
}

/// Read and parse a file fs_object (FsFileData) from the database.
pub async fn read_fs_file_data(
    repos: &Repositories,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsFileData, AppError> {
    let obj = repos
        .fs_object
        .find_by_repo_and_fs_id(repo_id, fs_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("fs_object not found: {fs_id}")))?;
    let data: FsFileData =
        serde_json::from_str(&obj.data).map_err(|e| AppError::internal(e.to_string()))?;
    Ok(data)
}

/// SQLite's variable limit (~999) caps `IN` clause width; chunk ID lists so
/// wide directories never trip it.
const IN_BATCH: usize = 500;

/// Fetch the given fs_objects in one batched `IN` query per chunk, keyed by
/// fs_id. Missing ids are simply absent from the map.
pub(crate) async fn fetch_fs_object_map(
    repos: &Repositories,
    repo_id: &str,
    fs_ids: &[String],
) -> Result<HashMap<String, fs_object::Model>, AppError> {
    let mut map = HashMap::with_capacity(fs_ids.len());
    for chunk in fs_ids.chunks(IN_BATCH) {
        for obj in repos
            .fs_object
            .find_by_repo_and_fs_ids(repo_id, chunk)
            .await?
        {
            map.insert(obj.fs_id.clone(), obj);
        }
    }
    Ok(map)
}

/// Batch version of `read_fs_dir_data` for a list of directory ids.
///
/// `EMPTY_SHA1` sentinels are skipped and non-directory / missing ids are
/// absent from the result map, mirroring the per-id semantics
/// (`Err(_) => continue`).
pub async fn read_fs_dir_data_batch(
    repos: &Repositories,
    repo_id: &str,
    fs_ids: &[String],
) -> Result<HashMap<String, FsDirData>, AppError> {
    let ids: Vec<String> = fs_ids
        .iter()
        .filter(|id| *id != EMPTY_SHA1)
        .cloned()
        .collect();
    let map = fetch_fs_object_map(repos, repo_id, &ids).await?;
    let mut out = HashMap::with_capacity(map.len());
    for (fs_id, obj) in map {
        if obj.obj_type != SEAF_METADATA_TYPE_DIR as i8 {
            continue;
        }
        let data: FsDirData =
            serde_json::from_str(&obj.data).map_err(|e| AppError::internal(e.to_string()))?;
        out.insert(fs_id, data);
    }
    Ok(out)
}

/// Batch version of `read_fs_file_data` for a list of file ids. Non-file ids
/// are absent from the result map.
pub async fn read_fs_file_data_batch(
    repos: &Repositories,
    repo_id: &str,
    fs_ids: &[String],
) -> Result<HashMap<String, FsFileData>, AppError> {
    let ids: Vec<String> = fs_ids
        .iter()
        .filter(|id| *id != EMPTY_SHA1)
        .cloned()
        .collect();
    let map = fetch_fs_object_map(repos, repo_id, &ids).await?;
    let mut out = HashMap::with_capacity(map.len());
    for (fs_id, obj) in map {
        if obj.obj_type != SEAF_METADATA_TYPE_FILE as i8 {
            continue;
        }
        let data: FsFileData =
            serde_json::from_str(&obj.data).map_err(|e| AppError::internal(e.to_string()))?;
        out.insert(fs_id, data);
    }
    Ok(out)
}

/// Traverse the FS tree from root_fs_id following path segments,
/// returning the fs_id of the final segment.
///
/// Path should be absolute (e.g. `/dir/subdir/file.txt`).
/// Returns the root_fs_id itself if path is "/" or empty.
pub async fn resolve_fs_id(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
    path: &str,
) -> Result<String, AppError> {
    let segments: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Ok(root_fs_id.to_string());
    }

    let mut current_fs_id = root_fs_id.to_string();

    for segment in segments {
        let dir_data = read_fs_dir_data(repos, repo_id, &current_fs_id).await?;

        let entry = dir_data
            .dirents
            .iter()
            .find(|d| d.name == segment)
            .ok_or_else(|| AppError::NotFound(format!("path segment not found: {segment}")))?;

        current_fs_id = entry.id.clone();
    }

    Ok(current_fs_id)
}

/// Resolve a file path to its `fs_id` and current `mtime` in a single walk.
///
/// The mtime is read from the final hop's parent dirent, so callers that need
/// both (e.g. thumbnail staleness checks) don't have to re-walk the tree or
/// re-read the parent directory. Error semantics match [`resolve_fs_id`]:
/// a missing segment yields `NotFound`; the empty path returns the root.
pub async fn resolve_file_entry(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
    path: &str,
) -> Result<(String, i64), AppError> {
    let segments: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Ok((root_fs_id.to_string(), 0));
    }

    let mut current_fs_id = root_fs_id.to_string();
    for segment in &segments[..segments.len() - 1] {
        let dir_data = read_fs_dir_data(repos, repo_id, &current_fs_id).await?;
        let entry = dir_data
            .dirents
            .iter()
            .find(|d| d.name == *segment)
            .ok_or_else(|| AppError::NotFound(format!("path segment not found: {segment}")))?;
        current_fs_id = entry.id.clone();
    }

    let last = segments.last().unwrap();
    let dir_data = read_fs_dir_data(repos, repo_id, &current_fs_id).await?;
    let entry = dir_data
        .dirents
        .iter()
        .find(|d| d.name == *last)
        .ok_or_else(|| AppError::NotFound(format!("path segment not found: {last}")))?;
    Ok((entry.id.clone(), entry.mtime))
}

/// Batch version of `resolve_fs_id` for many `(root_fs_id, path)` targets.
///
/// Resolves all targets in a shared level-frontier walk: at each depth it
/// deduplicates the "current directory" ids and fetches them in a single
/// batched query, so M paths of depth D cost O(D) queries instead of O(M·D).
/// Directories shared between targets are fetched only once.
///
/// The result is aligned with `targets`: `Some(fs_id)` for a fully resolved
/// path, `None` when any segment is missing (mirroring `resolve_fs_id`'s
/// `NotFound`), including a root or intermediate id that is `EMPTY_SHA1` or
/// not a directory.
pub async fn resolve_fs_ids_batch(
    repos: &Repositories,
    repo_id: &str,
    targets: &[(String, String)],
) -> Result<Vec<Option<String>>, AppError> {
    // Pre-parse each target's path into segments.
    let segments: Vec<Vec<String>> = targets
        .iter()
        .map(|(_, path)| {
            path.trim_start_matches('/')
                .split('/')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .collect();

    let mut results: Vec<Option<String>> = vec![None; targets.len()];

    struct Active {
        idx: usize,
        current_fs_id: String,
        next_segment: usize,
    }

    let mut active: Vec<Active> = Vec::new();
    for (i, segs) in segments.iter().enumerate() {
        if segs.is_empty() {
            results[i] = Some(targets[i].0.clone());
        } else {
            active.push(Active {
                idx: i,
                current_fs_id: targets[i].0.clone(),
                next_segment: 0,
            });
        }
    }

    while !active.is_empty() {
        let mut ids: Vec<String> = active.iter().map(|a| a.current_fs_id.clone()).collect();
        ids.sort();
        ids.dedup();

        let dir_map = read_fs_dir_data_batch(repos, repo_id, &ids).await?;

        let mut next: Vec<Active> = Vec::new();
        for a in active {
            let Some(dir_data) = dir_map.get(&a.current_fs_id) else {
                // Missing, EMPTY_SHA1, or not a directory → cannot descend.
                results[a.idx] = None;
                continue;
            };
            let segment = &segments[a.idx][a.next_segment];
            match dir_data.dirents.iter().find(|d| &d.name == segment) {
                Some(entry) => {
                    let next_segment = a.next_segment + 1;
                    if next_segment == segments[a.idx].len() {
                        results[a.idx] = Some(entry.id.clone());
                    } else {
                        next.push(Active {
                            idx: a.idx,
                            current_fs_id: entry.id.clone(),
                            next_segment,
                        });
                    }
                }
                None => {
                    results[a.idx] = None;
                }
            }
        }
        active = next;
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};
    use std::sync::Arc;

    async fn setup_tree_db() -> sea_orm::DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "CREATE TABLE fs_objects (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id VARCHAR(36) NOT NULL,
                fs_id VARCHAR(40) NOT NULL,
                obj_type TINYINT NOT NULL,
                data TEXT NOT NULL
            )",
        ))
        .await
        .unwrap();
        db
    }

    async fn insert(db: &sea_orm::DatabaseConnection, fs_id: &str, obj_type: i8, data: &str) {
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!(
                "INSERT INTO fs_objects (repo_id, fs_id, obj_type, data) \
                 VALUES ('r', '{fs_id}', {obj_type}, '{data}')"
            ),
        ))
        .await
        .unwrap();
    }

    /// root ── a(dirA) ── f1.txt(file1)
    ///      │            └── c(dirC)
    ///      └── b(dirB)
    async fn seed_tree(db: &sea_orm::DatabaseConnection) {
        insert(db, "root", 3, r#"{"dirents":[{"id":"dirA","mode":16384,"modifier":"","mtime":0,"name":"a","size":0},{"id":"dirB","mode":16384,"modifier":"","mtime":0,"name":"b","size":0}],"type":3,"version":1}"#).await;
        insert(db, "dirA", 3, r#"{"dirents":[{"id":"file1","mode":33188,"modifier":"u","mtime":0,"name":"f1.txt","size":10},{"id":"dirC","mode":16384,"modifier":"","mtime":0,"name":"c","size":0}],"type":3,"version":1}"#).await;
        insert(db, "dirB", 3, r#"{"dirents":[],"type":3,"version":1}"#).await;
        insert(
            db,
            "file1",
            1,
            r#"{"block_ids":["x"],"size":10,"type":1,"version":1}"#,
        )
        .await;
        insert(db, "dirC", 3, r#"{"dirents":[],"type":3,"version":1}"#).await;
    }

    #[tokio::test]
    async fn test_resolve_fs_ids_batch_resolves_and_shares() {
        let db = setup_tree_db().await;
        seed_tree(&db).await;
        let repos = Repositories::new(Arc::new(db));

        let targets = vec![
            ("root".to_string(), "/a/f1.txt".to_string()),
            ("root".to_string(), "/b".to_string()),
            ("root".to_string(), "/a/c".to_string()),
            ("root".to_string(), "/".to_string()),
            ("root".to_string(), "".to_string()),
        ];
        let got = resolve_fs_ids_batch(&repos, "r", &targets).await.unwrap();

        assert_eq!(
            got,
            vec![
                Some("file1".to_string()),
                Some("dirB".to_string()),
                Some("dirC".to_string()),
                Some("root".to_string()),
                Some("root".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn test_resolve_fs_ids_batch_missing_segments_are_none() {
        let db = setup_tree_db().await;
        seed_tree(&db).await;
        let repos = Repositories::new(Arc::new(db));

        let targets = vec![
            ("root".to_string(), "/nonexistent".to_string()),
            ("root".to_string(), "/a/nonexistent".to_string()),
            ("missing-root".to_string(), "/a".to_string()),
        ];
        let got = resolve_fs_ids_batch(&repos, "r", &targets).await.unwrap();

        assert_eq!(got, vec![None, None, None]);
    }

    #[tokio::test]
    async fn test_resolve_fs_ids_batch_empty_targets() {
        let db = setup_tree_db().await;
        let repos = Repositories::new(Arc::new(db));

        let got = resolve_fs_ids_batch(&repos, "r", &[]).await.unwrap();
        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn test_resolve_file_entry_returns_fs_id_and_mtime() {
        let db = setup_tree_db().await;
        // root ── photos ── pic.jpg (mtime 1700000000, non-zero to prove the
        // parent-dirent mtime is actually read through).
        insert(&db, "root", 3, r#"{"dirents":[{"id":"photos","mode":16384,"modifier":"","mtime":0,"name":"photos","size":0}],"type":3,"version":1}"#).await;
        insert(&db, "photos", 3, r#"{"dirents":[{"id":"pic","mode":33188,"modifier":"u","mtime":1700000000,"name":"pic.jpg","size":99}],"type":3,"version":1}"#).await;
        insert(
            &db,
            "pic",
            1,
            r#"{"block_ids":["x"],"size":99,"type":1,"version":1}"#,
        )
        .await;
        let repos = Repositories::new(Arc::new(db));

        let (fs_id, mtime) = resolve_file_entry(&repos, "r", "root", "/photos/pic.jpg")
            .await
            .unwrap();
        assert_eq!(fs_id, "pic");
        assert_eq!(mtime, 1700000000);

        // A directory entry resolves to its own fs_id + mtime from the parent.
        let (fs_id, mtime) = resolve_file_entry(&repos, "r", "root", "/photos")
            .await
            .unwrap();
        assert_eq!(fs_id, "photos");
        assert_eq!(mtime, 0);

        // Empty path returns the root.
        let (fs_id, _) = resolve_file_entry(&repos, "r", "root", "/").await.unwrap();
        assert_eq!(fs_id, "root");

        // Missing segment -> NotFound.
        let err = resolve_file_entry(&repos, "r", "root", "/photos/nope.jpg")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("path segment not found"));
    }
}
