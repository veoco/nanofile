//! Verification and repair of stored blocks (`nanofile verify-blocks`).
//!
//! Two independent checks:
//!
//! 1. Every stored block must hash to its content-addressed id. A mismatch, or
//!    a block that cannot be read at all, means the stored bytes are not the
//!    block they are named after: readers would serve the wrong bytes, and
//!    every file referencing the block is missing them.
//! 2. Every file object's `block_ids` must add up to the size it declares.
//!    Clients derive range/resume offsets from block sizes, so an object that
//!    does not add up makes them slice at the wrong offset.
//!
//! The report also counts file objects whose block sizes could not have come
//! from seafile's official chunker: those are files an official client will
//! re-chunk and re-upload once.
//!
//! `--repair` acts on check 1 only: a failing block is moved to
//! `{block_dir}/.quarantine/...` (bytes are never deleted) and removed from the
//! store, so clients see it as missing and a client holding a good copy
//! re-uploads it. Quarantined bytes are kept for forensics.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base::common::{
    EMPTY_SHA1, FsDirData, FsFileData, S_IFDIR, SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE,
};
use base::error::AppError;
use infra::crypto::fs_id::sha1_hex;
use infra::entity::{commit, fs_object, repo};
use infra::storage::DynBlockStorage;
use infra::storage::cdc::{SEAFILE_MAX_BLOCK, SEAFILE_MIN_BLOCK};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

/// Directory under the block root holding quarantined blocks. Outside
/// `repos/`, so the per-repository walks (and GC) never enumerate it.
const QUARANTINE_DIR: &str = ".quarantine";

/// One stored block whose bytes do not hash to its id, or that cannot be read.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CorruptBlock {
    pub repo_id: String,
    pub block_id: String,
    /// Stored length, when the block could be read at all.
    pub stored_len: Option<u64>,
    /// `sha1-mismatch`, or `unreadable: <error>`.
    pub reason: String,
}

/// A file object whose blocks cannot reproduce the size it declares.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileFinding {
    pub repo_id: String,
    pub fs_id: String,
    /// Path in the library's current tree, when the version is reachable from
    /// HEAD (`None` for a historical version that is no longer current).
    pub path: Option<String>,
    pub declared_size: i64,
    /// Sum of the stored blocks' logical sizes.
    pub stored_size: i64,
    pub blocks: usize,
    /// Blocks that are missing from the store entirely.
    pub missing_blocks: usize,
    /// Whether the block-size mix could not have come from the official chunker.
    pub non_official_chunking: bool,
}

/// What one verification pass found.
#[derive(Debug, Default, serde::Serialize)]
pub struct VerifyReport {
    pub repos_scanned: usize,
    pub blocks_checked: u64,
    pub file_objects_checked: u64,
    pub corrupt_blocks: Vec<CorruptBlock>,
    pub size_mismatch_files: Vec<FileFinding>,
    /// File objects whose blocks use non-official sizes (informational: an
    /// official client re-uploads them once, without any content loss).
    pub non_official_chunking_files: u64,
    pub quarantined_blocks: u64,
    /// Failures while quarantining; the corresponding block is still corrupt.
    pub repair_errors: Vec<String>,
}

impl VerifyReport {
    /// Whether the pass found anything an operator must act on.
    pub fn has_findings(&self) -> bool {
        !self.corrupt_blocks.is_empty()
            || !self.size_mismatch_files.is_empty()
            || !self.repair_errors.is_empty()
    }

    /// Print the report: the machine-readable form when `json`, a summary
    /// otherwise.
    pub fn print(&self, json: bool) {
        if json {
            match serde_json::to_string_pretty(self) {
                Ok(text) => println!("{text}"),
                Err(e) => eprintln!("could not serialize the report: {e}"),
            }
            return;
        }

        println!("libraries scanned:            {}", self.repos_scanned);
        println!("blocks checked:               {}", self.blocks_checked);
        println!("file objects checked:         {}", self.file_objects_checked);
        println!("corrupt blocks:               {}", self.corrupt_blocks.len());
        for block in &self.corrupt_blocks {
            let len = block
                .stored_len
                .map(|len| format!("{len} bytes"))
                .unwrap_or_else(|| "unreadable".to_string());
            println!(
                "  - {}/{} ({len}, {})",
                block.repo_id, block.block_id, block.reason
            );
        }
        println!(
            "file objects whose blocks do not add up: {}",
            self.size_mismatch_files.len()
        );
        for file in &self.size_mismatch_files {
            let where_ = file
                .path
                .clone()
                .unwrap_or_else(|| format!("(fs {} — not in the current tree)", file.fs_id));
            println!(
                "  - {}{} declares {} bytes, blocks hold {} ({} block(s), {} missing)",
                file.repo_id, where_, file.declared_size, file.stored_size, file.blocks,
                file.missing_blocks
            );
        }
        if self.non_official_chunking_files > 0 {
            println!(
                "file objects using non-official block sizes: {} \
                 (an official client re-uploads them once; no data loss)",
                self.non_official_chunking_files
            );
        }
        if self.quarantined_blocks > 0 {
            println!("quarantined blocks:           {}", self.quarantined_blocks);
        }
        for error in &self.repair_errors {
            println!("  ! {error}");
        }
    }
}

/// The `verify-blocks` command.
pub struct BlockVerify;

impl BlockVerify {
    /// Check every library (or `repo_filter`) and, when `repair`, quarantine the
    /// blocks that fail the id check.
    pub async fn run(
        db: &DatabaseConnection,
        store: &DynBlockStorage,
        block_dir: &Path,
        repo_filter: Option<&str>,
        repair: bool,
    ) -> Result<VerifyReport, AppError> {
        let repo_ids: Vec<String> = match repo_filter {
            Some(id) => vec![id.to_string()],
            None => store
                .repo_dirs()
                .await?
                .into_iter()
                .map(|(repo_id, _dir)| repo_id)
                .collect(),
        };

        let mut report = VerifyReport {
            repos_scanned: repo_ids.len(),
            ..Default::default()
        };

        // 1. Hash every stored block against its content-addressed id.
        let mut corrupt: Vec<CorruptBlock> = Vec::new();
        for repo_id in &repo_ids {
            for block_id in store.list_blocks(repo_id).await? {
                report.blocks_checked += 1;
                match store.read_block(repo_id, &block_id).await {
                    Ok(data) => {
                        if sha1_hex(&data) != block_id {
                            corrupt.push(CorruptBlock {
                                repo_id: repo_id.clone(),
                                block_id,
                                stored_len: Some(data.len() as u64),
                                reason: "sha1-mismatch".to_string(),
                            });
                        }
                    }
                    Err(e) => corrupt.push(CorruptBlock {
                        repo_id: repo_id.clone(),
                        block_id,
                        stored_len: None,
                        reason: format!("unreadable: {e}"),
                    }),
                }
            }
        }

        // 2. Check every file object against the store.
        for repo_id in &repo_ids {
            let paths = Self::current_paths(db, repo_id).await?;
            let files = fs_object::Entity::find()
                .filter(fs_object::Column::RepoId.eq(repo_id.as_str()))
                .filter(fs_object::Column::ObjType.eq(SEAF_METADATA_TYPE_FILE as i8))
                .all(db)
                .await?;

            for row in files {
                let file: FsFileData = match serde_json::from_str(&row.data) {
                    Ok(file) => file,
                    // Not a file object after all (or corrupt JSON): the
                    // commit path rejects those, so report and move on.
                    Err(_) => continue,
                };
                report.file_objects_checked += 1;

                let mut sizes = Vec::with_capacity(file.block_ids.len());
                let mut missing = 0usize;
                for id in &file.block_ids {
                    match store.block_size(repo_id, id).await {
                        Ok(size) => sizes.push(size),
                        Err(_) => missing += 1,
                    }
                }
                let stored_size: i64 = sizes.iter().sum();
                let non_official = Self::non_official_chunking(&sizes);
                if non_official {
                    report.non_official_chunking_files += 1;
                }
                if stored_size != file.size || missing > 0 {
                    report.size_mismatch_files.push(FileFinding {
                        repo_id: repo_id.clone(),
                        fs_id: row.fs_id.clone(),
                        path: paths.get(&row.fs_id).cloned(),
                        declared_size: file.size,
                        stored_size,
                        blocks: file.block_ids.len(),
                        missing_blocks: missing,
                        non_official_chunking: non_official,
                    });
                }
            }
        }

        // 3. Quarantine what failed the id check, so a client with a good copy
        //    re-uploads it and the file becomes readable again.
        if repair {
            for block in &corrupt {
                match Self::quarantine(store, block_dir, &block.repo_id, &block.block_id).await {
                    Ok(()) => report.quarantined_blocks += 1,
                    Err(e) => report.repair_errors.push(format!(
                        "could not quarantine {}/{}: {e}",
                        block.repo_id, block.block_id
                    )),
                }
            }
            store.invalidate_exists_cache();
        }

        corrupt.sort_by(|a, b| {
            (&a.repo_id, &a.block_id).cmp(&(&b.repo_id, &b.block_id))
        });
        report.corrupt_blocks = corrupt;
        Ok(report)
    }

    /// Whether a file's block sizes could not have come from seafile's official
    /// chunker.
    ///
    /// Official chunking emits blocks between the minimum and maximum size,
    /// except for the file's last block: every other block is at least
    /// [`SEAFILE_MIN_BLOCK`] and at most [`SEAFILE_MAX_BLOCK`]. A non-last block
    /// outside that range (the pre-2017 size-dependent chunker this server used
    /// before 2026-09-18 produced those) means an official client will re-chunk
    /// the file, find a different `seafile` object id and re-upload it once.
    fn non_official_chunking(sizes: &[i64]) -> bool {
        let Some((_last, rest)) = sizes.split_last() else {
            return false;
        };
        rest.iter()
            .any(|size| *size < SEAFILE_MIN_BLOCK as i64 || *size > SEAFILE_MAX_BLOCK as i64)
    }

    /// Map every file fs id in the library's current tree to its path. Versions
    /// only reachable from historical commits are reported without a path.
    async fn current_paths(
        db: &DatabaseConnection,
        repo_id: &str,
    ) -> Result<HashMap<String, String>, AppError> {
        let Some(repo) = repo::Entity::find_by_id(repo_id).one(db).await? else {
            return Ok(HashMap::new());
        };
        let Some(head) = repo.head_commit_id else {
            return Ok(HashMap::new());
        };
        let Some(commit) = commit::Entity::find()
            .filter(commit::Column::RepoId.eq(repo_id))
            .filter(commit::Column::CommitId.eq(head))
            .one(db)
            .await?
        else {
            return Ok(HashMap::new());
        };

        let mut paths = HashMap::new();
        let mut frontier: Vec<(String, String)> = vec![(commit.root_id, String::new())];
        let mut guard = crate::fs::core::traversal::TreeGuard::new();
        while !frontier.is_empty() {
            guard.enter_level()?;
            guard.visit(frontier.len())?;
            let ids: Vec<String> = frontier.iter().map(|(id, _)| id.clone()).collect();
            let dirs = Self::read_dirs(db, repo_id, &ids).await?;
            let mut next = Vec::new();
            for (dir_id, prefix) in &frontier {
                let Some(dir) = dirs.get(dir_id) else {
                    continue;
                };
                for entry in &dir.dirents {
                    let path = if prefix.is_empty() {
                        format!("/{}", entry.name)
                    } else {
                        format!("{prefix}/{}", entry.name)
                    };
                    if entry.mode & S_IFDIR != 0 {
                        if entry.id != EMPTY_SHA1 {
                            next.push((entry.id.clone(), path));
                        }
                    } else {
                        paths.insert(entry.id.clone(), path);
                    }
                }
            }
            frontier = next;
        }
        Ok(paths)
    }

    /// Fetch a batch of directory objects by id.
    async fn read_dirs(
        db: &DatabaseConnection,
        repo_id: &str,
        ids: &[String],
    ) -> Result<HashMap<String, FsDirData>, AppError> {
        let ids: Vec<String> = ids
            .iter()
            .filter(|id| id.as_str() != EMPTY_SHA1)
            .cloned()
            .collect();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.is_in(ids))
            .all(db)
            .await?;
        let mut out = HashMap::new();
        for row in rows {
            if row.obj_type != SEAF_METADATA_TYPE_DIR as i8 {
                continue;
            }
            if let Ok(dir) = serde_json::from_str::<FsDirData>(&row.data) {
                out.insert(row.fs_id, dir);
            }
        }
        Ok(out)
    }

    /// Move a failing block out of the store into the quarantine directory.
    ///
    /// The `store`'s id check already failed, so no reader can be relying on
    /// the bytes being the block they name; moving them makes the block read as
    /// missing, which is what makes a client re-upload it. The bytes themselves
    /// are kept for forensics.
    async fn quarantine(
        store: &DynBlockStorage,
        block_dir: &Path,
        repo_id: &str,
        block_id: &str,
    ) -> Result<(), AppError> {
        let repo_hash = sha1_hex(repo_id.as_bytes());
        let prefix = block_id.get(..2).unwrap_or(block_id);
        let source = block_dir
            .join(infra::storage::block_store::REPOS_DIR)
            .join(&repo_hash)
            .join(prefix)
            .join(block_id);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let target = block_dir
            .join(QUARANTINE_DIR)
            .join(&repo_hash)
            .join(format!("{block_id}.{stamp}"));

        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        match tokio::fs::rename(&source, &target).await {
            Ok(()) => {}
            // The block could not be read, so the file may already be gone:
            // there is nothing to preserve, but it must stop counting as
            // present.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AppError::internal(e.to_string())),
        }
        // Drops the presence-cache entry either way (`remove_block` tolerates a
        // block that is already gone).
        store.remove_block(repo_id, block_id).await?;
        Ok(())
    }
}

/// Open the block store the way the server does, so an admin command inspects
/// exactly the bytes a request would see (including the at-rest decorator).
pub fn open_block_store(config: &infra::config::Config) -> Result<DynBlockStorage, AppError> {
    use infra::storage::encrypting_block_store::{BlockEncryptionMode, EncryptingBlockStore};

    let mut store = infra::storage::new_block_store(&config.storage.block_dir);
    let mode = config.storage.block_encryption_mode();
    if mode != BlockEncryptionMode::Off {
        let key = config.storage.encryption_key.as_deref().ok_or_else(|| {
            AppError::Internal(
                "block at-rest encryption is enabled but no encryption key is configured".into(),
            )
        })?;
        let cipher = infra::crypto::block_encryption::BlockCipher::from_master_key(
            &crate::decode_master_key(key),
        );
        store = std::sync::Arc::new(EncryptingBlockStore::new(store, cipher, mode));
    }
    Ok(store)
}

/// Where a quarantined block is kept, for operator reference in the report.
pub fn quarantine_root(block_dir: &Path) -> PathBuf {
    block_dir.join(QUARANTINE_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use migration::MigratorTrait;
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    const REPO: &str = "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d";

    /// A real schema (the queries go through the entity mappings) with no rows.
    async fn temp_db() -> DatabaseConnection {
        // The fixture inserts file objects without their foreign-key parents
        // (repos/users), so FK enforcement is switched off for this connection.
        let mut opts = sea_orm::ConnectOptions::new("sqlite::memory:");
        opts.sqlx_logging(false)
            .map_sqlx_sqlite_opts(|o| o.foreign_keys(false));
        let db = sea_orm::Database::connect(opts)
            .await
            .expect("in-memory sqlite");
        migration::Migrator::up(&db, None)
            .await
            .expect("migrate");
        db
    }

    fn temp_block_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let blocks = dir.path().join("data").join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        (dir, blocks)
    }

    fn stored_block_path(block_dir: &Path, block_id: &str) -> PathBuf {
        block_dir
            .join(infra::storage::block_store::REPOS_DIR)
            .join(sha1_hex(REPO.as_bytes()))
            .join(&block_id[..2])
            .join(block_id)
    }

    async fn add_file_object(db: &DatabaseConnection, block_ids: &[String], size: i64) -> String {
        let list = block_ids
            .iter()
            .map(|id| format!("\"{id}\""))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(r#"{{"block_ids":[{list}],"size":{size},"type":1,"version":1}}"#);
        let fs_id = sha1_hex(json.as_bytes());
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT INTO fs_objects (repo_id, fs_id, obj_type, data) VALUES ($1, $2, 1, $3)",
            vec![REPO.into(), fs_id.clone().into(), json.into()],
        ))
        .await
        .expect("insert fs_object");
        fs_id
    }

    #[test]
    fn official_chunk_sizes_are_not_flagged() {
        // A break-sized block, then full-size blocks, then the tail.
        let sizes = vec![
            8_349_046,
            SEAFILE_MAX_BLOCK as i64,
            SEAFILE_MAX_BLOCK as i64,
            3_789_403,
        ];
        assert!(!BlockVerify::non_official_chunking(&sizes));
        // A single block of any size is the tail, so it is always official.
        assert!(!BlockVerify::non_official_chunking(&[1234]));
        assert!(!BlockVerify::non_official_chunking(&[]));
    }

    #[test]
    fn pre_2017_sized_blocks_are_flagged() {
        // The pre-2017 chunker's 1 MiB / 4 MiB blocks: a non-last block below
        // the 6 MiB minimum.
        assert!(BlockVerify::non_official_chunking(&[
            1_048_576,
            1_048_576,
            4_194_304
        ]));
        // A non-last block above the 10 MiB maximum cannot come from it either.
        assert!(BlockVerify::non_official_chunking(&[
            SEAFILE_MAX_BLOCK as i64 + 1,
            100
        ]));
    }

    #[tokio::test]
    async fn finds_and_quarantines_a_corrupt_block() {
        let db = temp_db().await;
        let (_dir, block_dir) = temp_block_dir();
        let store: DynBlockStorage = infra::storage::new_block_store(&block_dir);

        let good_id = store.write_block(REPO, b"good block").await.unwrap();
        let bad_id = store.write_block(REPO, b"bad block").await.unwrap();
        // Overwrite the second block's bytes: the file no longer hashes to its
        // own id, which is how a partial write used to look on disk.
        std::fs::write(stored_block_path(&block_dir, &bad_id), b"corrupted").unwrap();
        store.invalidate_exists_cache();

        // Report-only run: nothing is touched.
        let report = BlockVerify::run(&db, &store, &block_dir, None, false)
            .await
            .unwrap();
        assert_eq!(report.corrupt_blocks.len(), 1);
        assert_eq!(report.corrupt_blocks[0].block_id, bad_id);
        assert_eq!(report.corrupt_blocks[0].reason, "sha1-mismatch");
        assert!(report.has_findings());
        assert!(store.has_block(REPO, &bad_id).await);

        // Repair run: the block is quarantined (bytes kept) and reads missing,
        // so a client with a good copy re-uploads it.
        let report = BlockVerify::run(&db, &store, &block_dir, None, true)
            .await
            .unwrap();
        assert_eq!(report.quarantined_blocks, 1);
        assert!(
            !store.has_block(REPO, &bad_id).await,
            "a quarantined block must read as missing"
        );
        assert!(store.has_block(REPO, &good_id).await);
        let kept: Vec<_> = std::fs::read_dir(quarantine_root(&block_dir).join(sha1_hex(REPO.as_bytes())))
            .unwrap()
            .collect();
        assert_eq!(kept.len(), 1, "quarantined bytes must be preserved");
    }

    #[tokio::test]
    async fn reports_a_file_object_whose_blocks_do_not_add_up() {
        let db = temp_db().await;
        let (_dir, block_dir) = temp_block_dir();
        let store: DynBlockStorage = infra::storage::new_block_store(&block_dir);

        let data = b"sixteen bytes...";
        let block_id = store.write_block(REPO, data).await.unwrap();
        // The same block, but the object claims a size its bytes cannot reach.
        let fs_id = add_file_object(&db, std::slice::from_ref(&block_id), data.len() as i64 + 1).await;

        let report = BlockVerify::run(&db, &store, &block_dir, None, false)
            .await
            .unwrap();
        assert_eq!(report.file_objects_checked, 1);
        assert_eq!(report.size_mismatch_files.len(), 1);
        let finding = &report.size_mismatch_files[0];
        assert_eq!(finding.fs_id, fs_id);
        assert_eq!(finding.declared_size, data.len() as i64 + 1);
        assert_eq!(finding.stored_size, data.len() as i64);
        assert_eq!(finding.missing_blocks, 0);
        assert!(finding.path.is_none(), "no repo row in this fixture");

        // A consistent object is not reported.
        let _ = add_file_object(&db, std::slice::from_ref(&block_id), data.len() as i64).await;
        let report = BlockVerify::run(&db, &store, &block_dir, None, false)
            .await
            .unwrap();
        assert_eq!(report.file_objects_checked, 2);
        assert_eq!(report.size_mismatch_files.len(), 1);
    }
}
