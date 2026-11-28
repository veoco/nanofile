//! One-shot conversion of legacy blocks to versioned at-rest ciphertext.
//!
//! In `Lazy` mode new writes are encrypted but pre-existing blocks stay as
//! they are: either pre-encryption **plaintext**, or header-less **ciphertext**
//! written before the versioned (`NFE1 || key_id || ciphertext || tag`) header
//! existed. Both must converge before the server switches to `On`.
//!
//! A block is classified by probing the GCM-SIV tag: a body that decrypts is
//! ciphertext (re-encrypted, header only); anything else is plaintext and is
//! encrypted as-is. Block ids are `sha1(logical bytes)` and unchanged, so
//! Seafile clients, dedup and GC are unaffected.
//!
//! The pass is one-shot but not all-or-nothing: each repository it finishes is
//! appended to a progress log, and the completion marker is written only after
//! the last one. A restart, a cancel or a crash therefore redoes at most the
//! repository that was in flight, while a converted store short-circuits on the
//! marker without touching a block.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use base::error::AppError;
use infra::storage::{BlockStorageBackend, DynBlockStorage};

use crate::tasks::context::JobContext;
use crate::tasks::run::JobFailure;

/// Reported when a run is asked to stop at a checkpoint. The job body turns it
/// back into a cancellation, because only it knows which terminal state applies.
const ABORTED: &str = "block conversion stopped at a checkpoint";

/// What one conversion pass did, for the run record and the offline command.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConversionReport {
    /// The marker was already present: nothing was read or written.
    pub already_converted: bool,
    /// Repositories this pass walked, not counting the ones the log skipped.
    pub repos_scanned: usize,
    /// Repositories an earlier attempt had already finished.
    pub repos_skipped: usize,
    /// Blocks examined, converted or not.
    pub blocks_checked: u64,
    /// Legacy blocks rewritten as versioned ciphertext.
    pub blocks_converted: u64,
}

impl ConversionReport {
    pub fn summary(&self) -> String {
        if self.already_converted {
            return "block encryption conversion already complete".to_string();
        }
        let mut summary = format!(
            "block encryption conversion: {} repositories, {} blocks checked, {} converted",
            self.repos_scanned, self.blocks_checked, self.blocks_converted
        );
        if self.repos_skipped > 0 {
            summary.push_str(&format!(" ({} repositories resumed)", self.repos_skipped));
        }
        summary
    }
}

/// Where the store stands, derived without reading a single block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionStatus {
    pub converted: bool,
    /// Repositories the progress log names as finished.
    pub recorded_repos: usize,
    /// Repositories the log still expects.
    pub pending_repos: usize,
}

impl ConversionStatus {
    pub fn summary(&self) -> String {
        if self.converted {
            return "block encryption conversion complete".to_string();
        }
        format!(
            "block encryption conversion incomplete: {} repositories done, {} pending",
            self.recorded_repos, self.pending_repos
        )
    }
}

pub struct BlockEncryptionConverter;

impl BlockEncryptionConverter {
    /// Marker file name, stored directly in `block_dir`.
    const MARKER_FILE: &'static str = ".encryption_converted";
    /// Progress log: one finished repository id per line, removed when the pass ends.
    const PROGRESS_FILE: &'static str = ".encryption_convert.log";

    /// Blocks converted between checkpoints before the run's load budget scales
    /// it down, and the pause after each such batch.
    const BATCH_BLOCKS: usize = 100;
    const BATCH_SLEEP_MS: u64 = 100;
    /// Ids buffered ahead of the conversion. Fixed rather than budget-scaled so
    /// the channel is allocated once per repository.
    const WALK_DEPTH: usize = Self::BATCH_BLOCKS * 2;

    pub async fn is_converted(block_dir: &Path) -> bool {
        tokio::fs::try_exists(block_dir.join(Self::MARKER_FILE))
            .await
            .unwrap_or(false)
    }

    /// The marker, plus how many repositories the progress log records and how
    /// many a resumed pass would still walk.
    pub async fn status(
        block_store: &DynBlockStorage,
        block_dir: &Path,
    ) -> Result<ConversionStatus, AppError> {
        if Self::is_converted(block_dir).await {
            return Ok(ConversionStatus {
                converted: true,
                recorded_repos: 0,
                pending_repos: 0,
            });
        }
        let done = Self::read_progress(block_dir).await?;
        let repos = Self::repository_ids(block_store).await?;
        Ok(ConversionStatus {
            converted: false,
            recorded_repos: done.len(),
            pending_repos: repos.iter().filter(|repo| !done.contains(*repo)).count(),
        })
    }

    /// Convert every legacy block, in bounded batches with a short sleep between
    /// them, recording each finished repository so an interrupted pass resumes.
    ///
    /// `ctx` is the run's handle on the task system: with it the pass reports
    /// progress and checkpoints, so it yields to load and can be cancelled.
    /// Without it — the offline command and tests — it runs flat out.
    pub async fn convert_legacy_blocks(
        block_store: &DynBlockStorage,
        block_dir: &Path,
        ctx: Option<&JobContext>,
    ) -> Result<ConversionReport, AppError> {
        let mut report = ConversionReport::default();
        if Self::is_converted(block_dir).await {
            report.already_converted = true;
            return Ok(report);
        }

        let done = Self::read_progress(block_dir).await?;
        for repo_id in Self::repository_ids(block_store).await? {
            if done.contains(&repo_id) {
                report.repos_skipped += 1;
                continue;
            }
            report.repos_scanned += 1;
            Self::convert_repository(block_store, ctx, &repo_id, &mut report).await?;
            Self::record_progress(block_dir, &repo_id).await?;
        }

        Self::write_marker(block_dir).await?;
        if let Err(e) = tokio::fs::remove_file(block_dir.join(Self::PROGRESS_FILE)).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            // The marker is authoritative; a stale log is only misleading.
            tracing::warn!(
                path = %block_dir.join(Self::PROGRESS_FILE).display(),
                "could not remove the block conversion progress log: {e}"
            );
        }
        Ok(report)
    }

    /// Refuse to serve `on` mode from an unconverted store, and mark a store that
    /// has nothing to convert.
    ///
    /// `on` reads ciphertext only, so a leftover plaintext block is an unreadable
    /// file rather than a slow one. The marker is the only cheap record that every
    /// block was seen; without it the operator's remedy is the offline command.
    pub async fn prepare_at_rest_mode(config: &infra::config::Config) -> Result<(), String> {
        use infra::storage::encrypting_block_store::BlockEncryptionMode;

        let block_dir = &config.storage.block_dir;
        match config.storage.block_encryption_mode() {
            BlockEncryptionMode::Off => {
                if Self::is_converted(block_dir).await {
                    tracing::warn!(
                        "storage.block_encryption_mode = off but the block store was converted \
                         to at-rest ciphertext: off mode would serve the stored bytes as file \
                         content. Set the mode back to lazy or on."
                    );
                }
                Ok(())
            }
            BlockEncryptionMode::Lazy => Ok(()),
            BlockEncryptionMode::On => {
                if Self::is_converted(block_dir).await {
                    return Ok(());
                }
                let store = infra::storage::block_store::BlockStorage::new(block_dir.clone());
                let repos = store.repo_ids().await.map_err(|e| {
                    format!(
                        "cannot list the block store at {}: {e}",
                        block_dir.display()
                    )
                })?;
                if repos.is_empty() {
                    Self::write_marker(block_dir)
                        .await
                        .map_err(|e| e.to_string())?;
                    tracing::info!("empty block store marked converted for at-rest encryption");
                    return Ok(());
                }
                Err(format!(
                    "storage.block_encryption_mode = \"on\" but the block store at {} has not \
                     been converted, so pre-encryption blocks would be served unreadable. Run \
                     `nanofile convert-blocks` with the server stopped: it converts the blocks \
                     and writes {}. A store that has always run in \"on\" mode needs that \
                     command once; it verifies the blocks and writes the marker without \
                     rewriting any ciphertext.",
                    block_dir.display(),
                    block_dir.join(Self::MARKER_FILE).display()
                ))
            }
        }
    }

    /// Every repository with a block directory, in a stable order so a resumed
    /// pass works through the ones the log does not name.
    async fn repository_ids(block_store: &DynBlockStorage) -> Result<Vec<String>, AppError> {
        let mut repos = block_store
            .repo_ids()
            .await
            .map_err(|e| AppError::internal(format!("cannot list block repositories: {e}")))?;
        repos.sort_unstable();
        Ok(repos)
    }

    /// Convert one repository, handing the runtime back between batches.
    async fn convert_repository(
        block_store: &DynBlockStorage,
        ctx: Option<&JobContext>,
        repo_id: &str,
        report: &mut ConversionReport,
    ) -> Result<(), AppError> {
        // The walk streams ids through a bounded channel: enumerating a whole
        // repository into a Vec would pin one id string per block, and a large
        // library is exactly the case this pass exists for.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(Self::WALK_DEPTH);
        let walker = {
            let store = block_store.clone();
            let repo_id = repo_id.to_string();
            tokio::spawn(async move { store.stream_blocks_in_repo(&repo_id, tx).await })
        };

        let mut since_pause = 0usize;
        while let Some(block_id) = rx.recv().await {
            report.blocks_checked += 1;
            match block_store.convert_legacy_block(repo_id, &block_id).await {
                Ok(true) => report.blocks_converted += 1,
                Ok(false) => {}
                Err(e) => {
                    walker.abort();
                    return Err(AppError::internal(format!(
                        "cannot convert block {block_id} of repository {repo_id}: {e}"
                    )));
                }
            }
            since_pause += 1;
            if since_pause >= Self::pacing(ctx).0 {
                since_pause = 0;
                if let Err(e) = Self::pause(ctx, report).await {
                    walker.abort();
                    return Err(e);
                }
            }
        }

        walker
            .await
            .map_err(|e| AppError::internal(format!("block walk task failed: {e}")))?
            .map_err(|e| AppError::internal(format!("cannot walk repository {repo_id}: {e}")))?;
        Self::checkpoint(ctx).await?;
        Self::report(ctx, report);
        Ok(())
    }

    /// How many blocks to convert before handing the runtime back, and how long to
    /// sleep then. Scaled by the run's load budget so a busy server converts more
    /// slowly; without a run there is nobody to be polite to.
    fn pacing(ctx: Option<&JobContext>) -> (usize, Duration) {
        let Some(ctx) = ctx else {
            return (Self::BATCH_BLOCKS * 2, Duration::ZERO);
        };
        let budget = ctx.budget().clamp(0.05, 1.0);
        let batch = ((Self::BATCH_BLOCKS as f32 * budget) as usize).max(1);
        let sleep = Duration::from_millis((Self::BATCH_SLEEP_MS as f32 / budget) as u64);
        (batch, sleep)
    }

    /// The yield point of the pass: report what has been done, then sleep through a
    /// checkpoint. `JobContext::sleep` checkpoints on both sides, so the run parks
    /// while the server is busy and a cancel lands here instead of after the walk.
    async fn pause(ctx: Option<&JobContext>, report: &ConversionReport) -> Result<(), AppError> {
        let Some(ctx) = ctx else {
            return Ok(());
        };
        Self::report(Some(ctx), report);
        let (_, sleep) = Self::pacing(Some(ctx));
        ctx.sleep(sleep).await.map_err(Self::failure)
    }

    fn report(ctx: Option<&JobContext>, report: &ConversionReport) {
        if let Some(ctx) = ctx {
            // The total is unknown until the walk ends, so the run shows the count
            // of blocks examined rather than a percentage.
            ctx.report(report.blocks_checked, None);
        }
    }

    async fn checkpoint(ctx: Option<&JobContext>) -> Result<(), AppError> {
        let Some(ctx) = ctx else {
            return Ok(());
        };
        ctx.checkpoint().await.map_err(Self::failure)
    }

    /// The one mapping for every interruption point.
    fn failure(failure: JobFailure) -> AppError {
        match failure {
            JobFailure::Cancelled | JobFailure::TimedOut => {
                AppError::OperationFailed(ABORTED.into())
            }
            JobFailure::App(e) => e,
        }
    }

    /// Repositories an earlier attempt finished. An unreadable log is an error: it
    /// is the only resume record, so reading past it would silently redo the store.
    async fn read_progress(block_dir: &Path) -> Result<HashSet<String>, AppError> {
        match tokio::fs::read_to_string(block_dir.join(Self::PROGRESS_FILE)).await {
            Ok(text) => Ok(text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
            Err(e) => Err(AppError::internal(format!(
                "cannot read the block conversion progress log: {e}"
            ))),
        }
    }

    /// Record one finished repository. The append is not fsynced: a lost last line
    /// only makes the next attempt redo that repository.
    async fn record_progress(block_dir: &Path, repo_id: &str) -> Result<(), AppError> {
        use tokio::io::AsyncWriteExt;

        let path = block_dir.join(Self::PROGRESS_FILE);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| AppError::internal(format!("cannot open {}: {e}", path.display())))?;
        file.write_all(format!("{repo_id}\n").as_bytes())
            .await
            .map_err(|e| AppError::internal(format!("cannot write {}: {e}", path.display())))?;
        file.flush()
            .await
            .map_err(|e| AppError::internal(format!("cannot write {}: {e}", path.display())))
    }

    /// Record completion atomically: a crash mid-write must not leave a marker that
    /// claims a conversion the pass never finished.
    async fn write_marker(block_dir: &Path) -> Result<(), AppError> {
        let path = block_dir.join(Self::MARKER_FILE);
        let temp = block_dir.join(format!("{}.tmp", Self::MARKER_FILE));
        tokio::fs::write(&temp, b"done")
            .await
            .map_err(|e| AppError::internal(format!("cannot write {}: {e}", temp.display())))?;
        tokio::fs::rename(&temp, &path)
            .await
            .map_err(|e| AppError::internal(format!("cannot publish {}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use infra::crypto::block_encryption::BlockCipher;
    use infra::storage::BlockStorageBackend;
    use infra::storage::block_store::BlockStorage;
    use infra::storage::encrypting_block_store::{BlockEncryptionMode, EncryptingBlockStore};
    use std::sync::Arc;

    const MASTER: [u8; 32] = [0x42; 32];
    const REPO: &str = "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d";
    const OTHER: &str = "11111111-2222-3333-4444-555555555555";

    /// A lazy-mode decorator over a raw store sharing one temp dir, so tests can
    /// write legacy plaintext via the raw store and inspect on-disk ciphertext.
    fn temp_lazy_store() -> (
        tempfile::TempDir,
        DynBlockStorage,
        Arc<BlockStorage>,
        std::path::PathBuf,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let blocks = dir.path().join("data").join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        let raw = Arc::new(BlockStorage::new(blocks.clone()));
        let decorator: DynBlockStorage = Arc::new(EncryptingBlockStore::new(
            raw.clone(),
            BlockCipher::from_master_key(&MASTER),
            BlockEncryptionMode::Lazy,
        ));
        (dir, decorator, raw, blocks)
    }

    fn config_with(block_dir: &Path, mode: &str) -> infra::config::Config {
        let mut config = infra::config::Config::default();
        config.storage.block_dir = block_dir.to_path_buf();
        config.storage.block_encryption_mode = mode.to_string();
        config
    }

    #[tokio::test]
    async fn converts_plaintext_blocks_to_ciphertext() {
        let (_dir, store, raw, block_dir) = temp_lazy_store();

        let legacy = b"legacy plaintext block".to_vec();
        let legacy_id = raw.write_block(REPO, &legacy).await.unwrap();
        raw.invalidate_exists_cache();

        let report = BlockEncryptionConverter::convert_legacy_blocks(&store, &block_dir, None)
            .await
            .unwrap();
        assert_eq!(report.blocks_converted, 1);
        assert!(!report.already_converted);
        assert!(BlockEncryptionConverter::is_converted(&block_dir).await);

        // On-disk bytes are now `NFE1 || key_id || ciphertext || tag`.
        let on_disk = raw.read_block(REPO, &legacy_id).await.unwrap();
        assert_eq!(on_disk.len(), legacy.len() + 16 + 6);
    }

    #[tokio::test]
    async fn skips_blocks_already_encrypted() {
        let (_dir, store, raw, block_dir) = temp_lazy_store();

        let fresh = b"freshly encrypted block".to_vec();
        let fresh_id = store.write_block(REPO, &fresh).await.unwrap();
        let legacy = b"legacy plaintext block".to_vec();
        let legacy_id = raw.write_block(REPO, &legacy).await.unwrap();
        raw.invalidate_exists_cache();

        let report = BlockEncryptionConverter::convert_legacy_blocks(&store, &block_dir, None)
            .await
            .unwrap();
        // Only the plaintext block is counted as converted.
        assert_eq!(report.blocks_converted, 1);
        // The already-encrypted block is untouched (still decryptable).
        assert_eq!(store.read_block(REPO, &fresh_id).await.unwrap(), fresh);
        assert_eq!(
            raw.read_block(REPO, &legacy_id).await.unwrap().len(),
            legacy.len() + 16 + 6
        );
    }

    #[tokio::test]
    async fn converts_headerless_legacy_ciphertext_without_changing_content() {
        let (_dir, store, raw, block_dir) = temp_lazy_store();

        // Versioned ciphertext from before the header: 6-byte header stripped.
        let data = b"headerless legacy ciphertext block".to_vec();
        let id = store.write_block(REPO, &data).await.unwrap();
        let stored = raw.read_block(REPO, &id).await.unwrap();
        assert_eq!(&stored[..4], b"NFE1", "fixture must start versioned");
        raw.write_block_with_id_force(REPO, &id, &stored[6..])
            .await
            .unwrap();
        raw.invalidate_exists_cache();

        let report = BlockEncryptionConverter::convert_legacy_blocks(&store, &block_dir, None)
            .await
            .unwrap();
        assert_eq!(
            report.blocks_converted, 1,
            "a header-less ciphertext block needs conversion"
        );

        // Format change only: content and id unchanged; the versioned header is
        // back.
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);
        let on_disk = raw.read_block(REPO, &id).await.unwrap();
        assert_eq!(&on_disk[..4], b"NFE1");
        assert_eq!(on_disk.len(), data.len() + 16 + 6);
    }

    #[tokio::test]
    async fn a_converted_store_is_not_rescanned() {
        let (_dir, store, raw, block_dir) = temp_lazy_store();

        let first = b"legacy plaintext block".to_vec();
        let _first_id = raw.write_block(REPO, &first).await.unwrap();
        raw.invalidate_exists_cache();
        let report = BlockEncryptionConverter::convert_legacy_blocks(&store, &block_dir, None)
            .await
            .unwrap();
        assert_eq!(report.blocks_converted, 1);
        assert!(BlockEncryptionConverter::is_converted(&block_dir).await);

        // A block restored after the pass would be converted by a real scan. The
        // marker is authoritative: the pass must not even look at the store.
        let later = b"restored plaintext block".to_vec();
        let later_id = raw.write_block(REPO, &later).await.unwrap();
        raw.invalidate_exists_cache();

        let report = BlockEncryptionConverter::convert_legacy_blocks(&store, &block_dir, None)
            .await
            .unwrap();
        assert!(report.already_converted);
        assert_eq!(report.blocks_checked, 0);
        assert_eq!(report.blocks_converted, 0);
        assert_eq!(
            raw.read_block(REPO, &later_id).await.unwrap(),
            later,
            "the completion marker is authoritative"
        );
    }

    #[tokio::test]
    async fn a_pass_resumes_after_the_recorded_repositories() {
        let (_dir, store, raw, block_dir) = temp_lazy_store();

        let done_block = raw.write_block(OTHER, b"finished earlier").await.unwrap();
        let todo_block = raw.write_block(REPO, b"still to convert").await.unwrap();
        raw.invalidate_exists_cache();
        // Simulate an attempt that finished OTHER and stopped before REPO.
        std::fs::write(
            block_dir.join(".encryption_convert.log"),
            format!("{OTHER}\n"),
        )
        .unwrap();

        let report = BlockEncryptionConverter::convert_legacy_blocks(&store, &block_dir, None)
            .await
            .unwrap();
        assert_eq!(report.repos_skipped, 1);
        assert_eq!(report.repos_scanned, 1);
        assert_eq!(report.blocks_converted, 1);
        // The recorded repository is not revisited; the pending one is converted.
        assert_eq!(
            raw.read_block(OTHER, &done_block).await.unwrap(),
            b"finished earlier"
        );
        assert_eq!(
            &raw.read_block(REPO, &todo_block).await.unwrap()[..4],
            b"NFE1"
        );
        // A finished pass therefore leaves only the marker.
        assert!(BlockEncryptionConverter::is_converted(&block_dir).await);
        assert!(!block_dir.join(".encryption_convert.log").exists());
    }

    #[tokio::test]
    async fn status_reports_the_repositories_left() {
        let (_dir, store, raw, block_dir) = temp_lazy_store();
        raw.write_block(REPO, b"status").await.unwrap();
        raw.invalidate_exists_cache();

        let status = BlockEncryptionConverter::status(&store, &block_dir)
            .await
            .unwrap();
        assert!(!status.converted);
        assert_eq!(status.pending_repos, 1);
        assert_eq!(status.recorded_repos, 0);

        BlockEncryptionConverter::convert_legacy_blocks(&store, &block_dir, None)
            .await
            .unwrap();

        let status = BlockEncryptionConverter::status(&store, &block_dir)
            .await
            .unwrap();
        assert!(status.converted);
        assert_eq!(status.pending_repos, 0);
    }

    #[tokio::test]
    async fn is_converted_reflects_marker_file() {
        let dir = tempfile::tempdir().unwrap();
        let block_dir = dir.path().join("data").join("blocks");
        std::fs::create_dir_all(&block_dir).unwrap();

        assert!(!BlockEncryptionConverter::is_converted(&block_dir).await);
        tokio::fs::write(block_dir.join(".encryption_converted"), b"done")
            .await
            .unwrap();
        assert!(BlockEncryptionConverter::is_converted(&block_dir).await);
    }

    #[tokio::test]
    async fn a_finished_pass_leaves_no_temp_marker() {
        let (_dir, store, _raw, block_dir) = temp_lazy_store();
        BlockEncryptionConverter::convert_legacy_blocks(&store, &block_dir, None)
            .await
            .unwrap();
        assert!(block_dir.join(".encryption_converted").exists());
        assert!(!block_dir.join(".encryption_converted.tmp").exists());
    }

    #[tokio::test]
    async fn on_mode_marks_an_empty_store_converted() {
        let dir = tempfile::tempdir().unwrap();
        let block_dir = dir.path().join("blocks");
        std::fs::create_dir_all(&block_dir).unwrap();

        BlockEncryptionConverter::prepare_at_rest_mode(&config_with(&block_dir, "on"))
            .await
            .unwrap();
        assert!(BlockEncryptionConverter::is_converted(&block_dir).await);
    }

    #[tokio::test]
    async fn on_mode_refuses_an_unconverted_store_with_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let block_dir = dir.path().join("blocks");
        std::fs::create_dir_all(&block_dir).unwrap();
        Arc::new(BlockStorage::new(block_dir.clone()))
            .write_block(REPO, b"x")
            .await
            .unwrap();

        let error = BlockEncryptionConverter::prepare_at_rest_mode(&config_with(&block_dir, "on"))
            .await
            .unwrap_err();
        assert!(error.contains("convert-blocks"), "{error}");
        assert!(!BlockEncryptionConverter::is_converted(&block_dir).await);
    }

    #[tokio::test]
    async fn on_mode_accepts_a_marked_store() {
        let dir = tempfile::tempdir().unwrap();
        let block_dir = dir.path().join("blocks");
        std::fs::create_dir_all(&block_dir).unwrap();
        Arc::new(BlockStorage::new(block_dir.clone()))
            .write_block(REPO, b"x")
            .await
            .unwrap();
        tokio::fs::write(block_dir.join(".encryption_converted"), b"done")
            .await
            .unwrap();

        BlockEncryptionConverter::prepare_at_rest_mode(&config_with(&block_dir, "on"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn lazy_and_off_modes_are_never_refused() {
        let dir = tempfile::tempdir().unwrap();
        let block_dir = dir.path().join("blocks");
        std::fs::create_dir_all(&block_dir).unwrap();
        Arc::new(BlockStorage::new(block_dir.clone()))
            .write_block(REPO, b"x")
            .await
            .unwrap();

        for mode in ["lazy", "off"] {
            BlockEncryptionConverter::prepare_at_rest_mode(&config_with(&block_dir, mode))
                .await
                .unwrap_or_else(|e| panic!("{mode} must not be refused: {e}"));
        }
    }
}
