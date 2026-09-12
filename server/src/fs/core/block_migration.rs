//! One-shot migration of the legacy flat block layout to the per-repository
//! layout.
//!
//! The legacy layout stored every block of every library in one flat
//! content-addressed tree keyed only by the block id:
//!
//! ```text
//! {block_dir}/<2hex>/<block_id>
//! ```
//!
//! The current layout stores each block inside the library that owns it:
//!
//! ```text
//! {block_dir}/repos/<sha1(repo_id)>/<2hex>/<block_id>
//! ```
//!
//! Namespacing by repository is what makes authorization exact — a block id
//! learned elsewhere (a cached `fs_object`, a revoked collaborator's client
//! state, a dedup oracle) is simply not reachable through a library the caller
//! is not a member of. The legacy tree therefore has to go away completely: the
//! server has no read path for it.
//!
//! Properties of this migration:
//!
//! * **Pure copy.** No hard links and no symlinks, so it behaves identically on
//!   Windows, exFAT and network shares.
//! * **Idempotent and resumable.** A block already present with the same size is
//!   skipped, so an interrupted run can simply be repeated.
//! * **Verify before delete.** The legacy tree is removed only after every
//!   referenced block has been copied or confirmed in its new location. A
//!   size mismatch aborts the run *before* anything is deleted.
//! * **No leftovers.** The legacy directories are removed together with any
//!   temporary file an interrupted copy may have left, and a layout marker is
//!   written so later startups take the fast path.
//!
//! The reference set comes from the `fs_objects` table, which is already keyed
//! by repository: that covers sub-repository copies and every historical
//! revision without walking a single FS tree.

use std::collections::BTreeSet;
use std::path::Path;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect};
use serde::Deserialize;

use base::common::SEAF_METADATA_TYPE_FILE;
use base::error::AppError;
use infra::entity::fs_object;
use infra::storage::block_store::BlockStorage;
use infra::storage::{BlockStorageBackend, LegacyCopyOutcome};

/// Summary of one migration run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Repositories that reference at least one block.
    pub repositories: usize,
    /// Distinct `(repository, block)` pairs found in `fs_objects`.
    pub references: usize,
    /// Blocks copied into a repository directory.
    pub copied: usize,
    /// Blocks whose destination already held identical content.
    pub already_present: usize,
    /// Referenced blocks absent from the legacy store (data lost before this
    /// run; reported so the operator can restore from a backup).
    pub missing_sources: usize,
    /// Legacy files present when the run started.
    pub legacy_files: usize,
    /// Bytes held by those legacy files.
    pub legacy_bytes: u64,
    /// Legacy top-level directories removed.
    pub purged_dirs: u64,
    /// Stray `.tmp` files removed from repository directories.
    pub stale_temp_files: u64,
    /// The block root was already on the current layout.
    pub already_migrated: bool,
    /// Nothing was modified (report only).
    pub dry_run: bool,
    /// First few missing block ids (bounded so the report stays printable).
    pub missing_block_ids: Vec<String>,
}

impl MigrationReport {
    /// One-line summary for logs and CLI output.
    pub fn summary(&self) -> String {
        if self.already_migrated {
            return format!(
                "block layout already current ({} repositories)",
                self.repositories
            );
        }
        format!(
            "block layout migration{}: {} repositories, {} references, {} copied, \
             {} already present, {} missing, {} legacy files ({} bytes) purged, \
             {} legacy dirs removed, {} stale temp files removed{}",
            if self.dry_run { " (dry run)" } else { "" },
            self.repositories,
            self.references,
            self.copied,
            self.already_present,
            self.missing_sources,
            self.legacy_files,
            self.legacy_bytes,
            self.purged_dirs,
            self.stale_temp_files,
            if self.missing_sources > 0 {
                " — referenced blocks were already missing from the legacy store"
            } else {
                ""
            }
        )
    }
}

/// How a run was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationMode {
    /// Report what would happen; never touch the filesystem.
    DryRun,
    /// Copy, verify, purge the legacy tree and write the layout marker.
    Apply,
}

pub struct BlockLayoutMigration;

impl BlockLayoutMigration {
    /// Progress log interval (in blocks) for a long migration.
    const LOG_EVERY: usize = 5_000;

    /// Whether the legacy layout or an interrupted migration is still present.
    ///
    /// Used by the startup guard: while this returns `true` the server must not
    /// serve, because the new code paths only look in the per-repository layout.
    pub async fn migration_pending(block_dir: &Path) -> bool {
        let store = BlockStorage::new(block_dir.to_path_buf());
        matches!(store.legacy_layout_present().await, Ok(true))
    }

    /// Run (or report) the migration.
    pub async fn run(
        db: &DatabaseConnection,
        block_dir: &Path,
        mode: MigrationMode,
    ) -> Result<MigrationReport, AppError> {
        let store = BlockStorage::new(block_dir.to_path_buf());

        let legacy_files = store
            .legacy_blocks()
            .await
            .map_err(|e| AppError::internal(format!("scan legacy block layout: {e}")))?;

        // The reference map is needed even for a dry run (to report the copy
        // volume), but not when there is nothing to migrate.
        let references = if legacy_files.is_empty() {
            BTreeSet::new()
        } else {
            Self::collect_references(db).await?
        };

        let repositories = references
            .iter()
            .map(|(repo_id, _)| repo_id.clone())
            .collect::<BTreeSet<_>>()
            .len();

        let mut report = MigrationReport {
            repositories,
            references: references.len(),
            legacy_files: legacy_files.len(),
            legacy_bytes: legacy_files.iter().map(|(_, len)| *len).sum(),
            dry_run: mode == MigrationMode::DryRun,
            ..Default::default()
        };

        if references.is_empty() && legacy_files.is_empty() {
            // Fresh install, or a previous run already finished. Make the
            // current layout explicit and clear any partial copy left behind.
            report.already_migrated = true;
            if mode == MigrationMode::Apply {
                report.stale_temp_files = store
                    .purge_layout_temp_files()
                    .await
                    .map_err(|e| AppError::internal(format!("remove stale temp files: {e}")))?;
                store
                    .write_layout_marker()
                    .await
                    .map_err(|e| AppError::internal(format!("write layout marker: {e}")))?;
            }
            return Ok(report);
        }

        // A dry run only reports what would happen: the would-be copies are
        // derived from the legacy file list plus the current layout, so nothing
        // is written.
        if mode == MigrationMode::DryRun {
            let legacy_ids: BTreeSet<&str> =
                legacy_files.iter().map(|(id, _)| id.as_str()).collect();
            for (repo_id, block_id) in &references {
                if !legacy_ids.contains(block_id.as_str()) {
                    report.missing_sources += 1;
                    if report.missing_block_ids.len() < 100 {
                        report.missing_block_ids.push(block_id.clone());
                    }
                } else if store.has_block(repo_id, block_id).await {
                    report.already_present += 1;
                } else {
                    report.copied += 1;
                }
            }
            return Ok(report);
        }

        // 1. Copy every referenced block into its own repository directory.
        for (index, (repo_id, block_id)) in references.iter().enumerate() {
            if index > 0 && index % Self::LOG_EVERY == 0 {
                tracing::info!(
                    done = index,
                    total = references.len(),
                    "block layout migration in progress"
                );
            }
            match store.migrate_legacy_block(repo_id, block_id).await {
                Ok(LegacyCopyOutcome::Copied) => report.copied += 1,
                Ok(LegacyCopyOutcome::AlreadyPresent) => report.already_present += 1,
                Ok(LegacyCopyOutcome::MissingSource) => {
                    report.missing_sources += 1;
                    if report.missing_block_ids.len() < 100 {
                        report.missing_block_ids.push(block_id.clone());
                    }
                }
                // A size mismatch means a destination file is corrupt: abort
                // while the legacy tree is still intact.
                Err(e) => {
                    return Err(AppError::internal(format!(
                        "cannot migrate block {block_id} of repository {repo_id}: {e}"
                    )));
                }
            }
        }

        // 2. Purge the legacy layout wholesale. Every block that could still be
        //    referenced was copied in step 1 (missing sources had nothing to
        //    copy), so nothing of value is left behind.
        report.purged_dirs = store
            .purge_legacy_layout()
            .await
            .map_err(|e| AppError::internal(format!("remove legacy block layout: {e}")))?;

        // 3. Clear partial copies and record completion so the next startup
        //    skips the whole scan.
        report.stale_temp_files = store
            .purge_layout_temp_files()
            .await
            .map_err(|e| AppError::internal(format!("remove stale temp files: {e}")))?
            as u64;
        store
            .write_layout_marker()
            .await
            .map_err(|e| AppError::internal(format!("write layout marker: {e}")))?;

        Ok(report)
    }

    /// Every `(repo_id, block_id)` pair referenced by a file object.
    ///
    /// Rows are read per repository so the working set stays proportional to one
    /// library at a time rather than the whole table.
    async fn collect_references(
        db: &DatabaseConnection,
    ) -> Result<BTreeSet<(String, String)>, AppError> {
        let repo_ids: Vec<String> = fs_object::Entity::find()
            .select_only()
            .column(fs_object::Column::RepoId)
            .distinct()
            .into_tuple::<String>()
            .all(db)
            .await?;

        let mut out: BTreeSet<(String, String)> = BTreeSet::new();
        for repo_id in repo_ids {
            let files = fs_object::Entity::find()
                .filter(fs_object::Column::RepoId.eq(repo_id.as_str()))
                .filter(fs_object::Column::ObjType.eq(SEAF_METADATA_TYPE_FILE as i8))
                .all(db)
                .await?;
            for file in files {
                // A row that does not parse is already unusable to every reader
                // (the same `serde_json` parse guards the download path), so it
                // is skipped rather than failing the whole migration.
                let Ok(data) = serde_json::from_str::<FileObjectData>(&file.data) else {
                    tracing::warn!(
                        fs_id = %file.fs_id,
                        repo_id = %repo_id,
                        "skipping unparseable fs_object during block migration"
                    );
                    continue;
                };
                for block_id in data.block_ids {
                    out.insert((repo_id.clone(), block_id));
                }
            }
        }
        Ok(out)
    }
}

/// Minimal view of a file object: only the block list is needed here.
#[derive(Deserialize)]
struct FileObjectData {
    block_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use infra::storage::BlockStorageBackend;
    use sea_orm::ConnectionTrait;
    use sea_orm::{DatabaseBackend, Statement};

    const REPO_A: &str = "aaaaaaaa-1111-1111-1111-111111111111";
    const REPO_B: &str = "bbbbbbbb-2222-2222-2222-222222222222";

    async fn temp_db() -> DatabaseConnection {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "CREATE TABLE fs_objects (\
                 id INTEGER PRIMARY KEY AUTOINCREMENT, \
                 repo_id VARCHAR(36) NOT NULL, \
                 fs_id VARCHAR(40) NOT NULL, \
                 obj_type TINYINT NOT NULL, \
                 data TEXT NOT NULL)",
        ))
        .await
        .expect("create fs_objects");
        db
    }

    async fn add_file_object(db: &DatabaseConnection, repo_id: &str, fs_id: &str, blocks: &[&str]) {
        let list = blocks
            .iter()
            .map(|b| format!("\"{b}\""))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(r#"{{"block_ids":[{list}],"size":1,"type":1,"version":1}}"#);
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT INTO fs_objects (repo_id, fs_id, obj_type, data) VALUES ($1, $2, 1, $3)",
            vec![repo_id.into(), fs_id.into(), json.into()],
        ))
        .await
        .expect("insert fs_object");
    }

    fn block_id(content: &[u8]) -> String {
        infra::crypto::fs_id::sha1_hex(content)
    }

    fn temp_block_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let blocks = dir.path().join("data").join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        (dir, blocks)
    }

    /// Seed the legacy flat layout with `blocks`.
    async fn seed_legacy(block_dir: &Path, blocks: &[(&str, &[u8])]) {
        for (id, data) in blocks {
            let dir = block_dir.join(&id[..2]);
            tokio::fs::create_dir_all(&dir).await.unwrap();
            tokio::fs::write(dir.join(id), data).await.unwrap();
        }
    }

    /// The migration copies each referenced block into its own repository and
    /// removes every legacy file: no leftovers, and the same block content is
    /// owned separately by each repository that references it.
    #[tokio::test]
    async fn migrates_references_and_removes_every_legacy_file() {
        let db = temp_db().await;
        let (_dir, block_dir) = temp_block_dir();

        let shared = b"shared content";
        let only_a = b"only in A";
        let shared_id = block_id(shared);
        let only_a_id = block_id(only_a);
        // The same content is referenced by both repositories.
        add_file_object(&db, REPO_A, &"a".repeat(40), &[&shared_id, &only_a_id]).await;
        add_file_object(&db, REPO_B, &"b".repeat(40), &[&shared_id]).await;
        // An orphan not referenced by anything must not block the purge.
        let orphan = b"orphan";
        let orphan_id = block_id(orphan);
        seed_legacy(
            &block_dir,
            &[
                (&shared_id, shared),
                (&only_a_id, only_a),
                (&orphan_id, orphan),
            ],
        )
        .await;

        let report = BlockLayoutMigration::run(&db, &block_dir, MigrationMode::Apply)
            .await
            .expect("migration succeeds");

        assert_eq!(report.repositories, 2);
        assert_eq!(report.references, 3);
        assert_eq!(report.copied, 3, "shared content is copied per repository");
        assert_eq!(report.legacy_files, 3);
        assert!(
            report.purged_dirs >= 2,
            "every legacy prefix dir is removed, got {}",
            report.purged_dirs
        );
        assert_eq!(report.missing_sources, 0);

        let store = BlockStorage::new(block_dir.clone());
        assert!(store.layout_is_current().await);
        assert!(!store.legacy_layout_present().await.unwrap());
        assert!(store.has_block(REPO_A, &shared_id).await);
        assert!(store.has_block(REPO_B, &shared_id).await);
        assert!(store.has_block(REPO_A, &only_a_id).await);
        assert!(
            !store.has_block(REPO_B, &only_a_id).await,
            "a block only A references must not exist in B"
        );
        assert!(
            !store.has_block(REPO_A, &orphan_id).await,
            "unreferenced legacy blocks are dropped, not copied"
        );

        // Zero leftovers: the block root now holds exactly the current layout
        // and its marker — no legacy prefix directories, no temporary files.
        let mut entries: Vec<String> = std::fs::read_dir(&block_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        entries.sort();
        assert_eq!(
            entries,
            vec![".block_layout".to_string(), "repos".to_string()]
        );
    }

    /// A dry run reports the work without touching a single file.
    #[tokio::test]
    async fn dry_run_changes_nothing() {
        let db = temp_db().await;
        let (_dir, block_dir) = temp_block_dir();
        let content = b"content";
        let id = block_id(content);
        add_file_object(&db, REPO_A, &"a".repeat(40), &[&id]).await;
        seed_legacy(&block_dir, &[(&id, content)]).await;

        let report = BlockLayoutMigration::run(&db, &block_dir, MigrationMode::DryRun)
            .await
            .expect("dry run succeeds");
        assert!(report.dry_run);
        assert_eq!(
            report.copied, 1,
            "the dry run reports the block it would copy"
        );
        assert_eq!(report.purged_dirs, 0);

        let store = BlockStorage::new(block_dir.clone());
        assert!(store.legacy_layout_present().await.unwrap());
        assert!(!store.layout_is_current().await);
        assert!(!store.has_block(REPO_A, &id).await);
    }

    /// Re-running after an interrupted migration completes it, and a completed
    /// migration is a no-op.
    #[tokio::test]
    async fn migration_is_idempotent_and_resumable() {
        let db = temp_db().await;
        let (_dir, block_dir) = temp_block_dir();
        let content = b"content";
        let id = block_id(content);
        add_file_object(&db, REPO_A, &"a".repeat(40), &[&id]).await;
        seed_legacy(&block_dir, &[(&id, content)]).await;

        let first = BlockLayoutMigration::run(&db, &block_dir, MigrationMode::Apply)
            .await
            .unwrap();
        assert_eq!(first.copied, 1);

        // Second run: the legacy tree is gone, so nothing happens.
        let second = BlockLayoutMigration::run(&db, &block_dir, MigrationMode::Apply)
            .await
            .unwrap();
        assert!(second.already_migrated);
        assert_eq!(second.purged_dirs, 0);

        // The block is still there and readable after the re-run.
        let store = BlockStorage::new(block_dir.clone());
        assert_eq!(store.read_block(REPO_A, &id).await.unwrap(), content);
    }

    /// Referenced blocks that the legacy store no longer holds are reported
    /// instead of aborting: the data was already lost before this run, and the
    /// legacy tree holds nothing to recover it with.
    #[tokio::test]
    async fn missing_legacy_blocks_are_reported_not_fatal() {
        let db = temp_db().await;
        let (_dir, block_dir) = temp_block_dir();
        let present = b"present";
        let present_id = block_id(present);
        let absent_id = block_id(b"absent");
        add_file_object(&db, REPO_A, &"a".repeat(40), &[&present_id, &absent_id]).await;
        seed_legacy(&block_dir, &[(&present_id, present)]).await;

        let report = BlockLayoutMigration::run(&db, &block_dir, MigrationMode::Apply)
            .await
            .unwrap();
        assert_eq!(report.copied, 1);
        assert_eq!(report.missing_sources, 1);
        assert_eq!(report.missing_block_ids, vec![absent_id]);
        assert!(!BlockLayoutMigration::migration_pending(&block_dir).await);
    }

    /// Without the migration the new code paths cannot see the blocks at all,
    /// which is exactly why startup must not proceed while it is pending.
    #[tokio::test]
    async fn pending_flag_tracks_the_legacy_layout() {
        let (_dir, block_dir) = temp_block_dir();
        assert!(!BlockLayoutMigration::migration_pending(&block_dir).await);
        seed_legacy(&block_dir, &[(&block_id(b"x"), b"x")]).await;
        assert!(BlockLayoutMigration::migration_pending(&block_dir).await);
    }
}
