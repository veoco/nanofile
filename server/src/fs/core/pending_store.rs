use async_trait::async_trait;
use infra::storage::{BlockStorageBackend, DynBlockStorage};
use std::sync::Arc;

use crate::repository::pending_block::PendingBlockRepository;

/// Records every upload block in the durable pending ledger before it is written.
///
/// The ledger is what lets garbage collection find an interrupted upload's blocks
/// after a restart, and what answers "is this block newer than the grace window"
/// without asking the store once per block. Only the *tracked* write is marked:
/// it is the one every upload path uses (a commit follows), while the force write
/// belongs to the lazy→encrypted conversion and the untracked write to operations
/// that touch no upload.
///
/// The wrapper sits above the at-rest encryption decorator, so the id it records is
/// the logical one the commit and the collector use.
pub struct PendingBlockStore {
    inner: DynBlockStorage,
    pending: Arc<dyn PendingBlockRepository>,
}

impl PendingBlockStore {
    pub fn new(inner: DynBlockStorage, pending: Arc<dyn PendingBlockRepository>) -> Self {
        Self { inner, pending }
    }

    /// Announce a block before writing it.
    ///
    /// A failure is logged and the write proceeds: the commit that follows protects
    /// the block anyway, and failing an upload over a ledger hiccup would be worse
    /// than the narrow leak (a crash before the commit) the audit pass still finds.
    async fn mark(&self, repo_id: &str, block_id: &str) {
        let now = chrono::Utc::now().timestamp();
        if let Err(e) = self
            .pending
            .mark_written(repo_id, std::slice::from_ref(&block_id.to_string()), now)
            .await
        {
            tracing::warn!(
                repo_id,
                block_id,
                "pending-block ledger write failed, continuing without the restart guard: {e}"
            );
        }
    }

    /// The write reported the block already existed, or failed: either way this
    /// upload no longer needs the restart guard for it.
    async fn forget(&self, repo_id: &str, block_id: &str) {
        if let Err(e) = self
            .pending
            .forget(repo_id, std::slice::from_ref(&block_id.to_string()))
            .await
        {
            tracing::debug!(repo_id, block_id, "could not drop a pending-block row: {e}");
        }
    }
}

impl std::fmt::Debug for PendingBlockStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingBlockStore").finish_non_exhaustive()
    }
}

#[async_trait]
impl BlockStorageBackend for PendingBlockStore {
    async fn has_block(&self, repo_id: &str, block_id: &str) -> bool {
        self.inner.has_block(repo_id, block_id).await
    }

    async fn read_block(&self, repo_id: &str, block_id: &str) -> Result<Vec<u8>, std::io::Error> {
        self.inner.read_block(repo_id, block_id).await
    }

    async fn read_block_prefix(
        &self,
        repo_id: &str,
        block_id: &str,
        max: usize,
    ) -> Result<Vec<u8>, std::io::Error> {
        self.inner.read_block_prefix(repo_id, block_id, max).await
    }

    async fn write_block(&self, repo_id: &str, data: &[u8]) -> Result<String, std::io::Error> {
        self.inner.write_block(repo_id, data).await
    }

    async fn write_block_with_id(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<String, std::io::Error> {
        self.inner
            .write_block_with_id(repo_id, block_id, data)
            .await
    }

    /// The one method this wrapper exists for: mark first, then write, then drop the
    /// marker if the block turned out to be a deduplicated write.
    async fn write_block_with_id_tracked(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<(String, bool), std::io::Error> {
        self.mark(repo_id, block_id).await;
        match self
            .inner
            .write_block_with_id_tracked(repo_id, block_id, data)
            .await
        {
            Ok((id, true)) => Ok((id, true)),
            Ok((id, false)) => {
                self.forget(repo_id, &id).await;
                Ok((id, false))
            }
            Err(e) => {
                self.forget(repo_id, block_id).await;
                Err(e)
            }
        }
    }

    async fn write_block_with_id_force(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<String, std::io::Error> {
        self.inner
            .write_block_with_id_force(repo_id, block_id, data)
            .await
    }

    async fn convert_legacy_block(
        &self,
        repo_id: &str,
        block_id: &str,
    ) -> Result<bool, std::io::Error> {
        self.inner.convert_legacy_block(repo_id, block_id).await
    }

    async fn remove_block(&self, repo_id: &str, block_id: &str) -> Result<(), std::io::Error> {
        self.inner.remove_block(repo_id, block_id).await
    }

    async fn remove_blocks(
        &self,
        repo_id: &str,
        block_ids: &[String],
    ) -> Result<u64, std::io::Error> {
        self.inner.remove_blocks(repo_id, block_ids).await
    }

    async fn block_size(&self, repo_id: &str, block_id: &str) -> Result<i64, std::io::Error> {
        self.inner.block_size(repo_id, block_id).await
    }

    fn logical_sizes_are_exact(&self) -> bool {
        self.inner.logical_sizes_are_exact()
    }

    async fn block_modified_secs(
        &self,
        repo_id: &str,
        block_id: &str,
    ) -> Result<Option<i64>, std::io::Error> {
        self.inner.block_modified_secs(repo_id, block_id).await
    }

    async fn list_blocks(&self, repo_id: &str) -> Result<Vec<String>, std::io::Error> {
        self.inner.list_blocks(repo_id).await
    }

    async fn for_each_block_in_repo(
        &self,
        repo_id: &str,
        f: Box<dyn for<'a> FnMut(&'a str) + Send>,
    ) -> Result<(), std::io::Error> {
        self.inner.for_each_block_in_repo(repo_id, f).await
    }

    async fn for_each_block_with_mtime(
        &self,
        repo_id: &str,
        f: Box<dyn for<'a> FnMut(&'a str, Option<i64>) + Send>,
    ) -> Result<(), std::io::Error> {
        self.inner.for_each_block_with_mtime(repo_id, f).await
    }

    async fn stream_blocks_in_repo(
        &self,
        repo_id: &str,
        tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<(), std::io::Error> {
        self.inner.stream_blocks_in_repo(repo_id, tx).await
    }

    async fn repo_dirs(&self) -> Result<Vec<(String, std::path::PathBuf)>, std::io::Error> {
        self.inner.repo_dirs().await
    }

    async fn repo_ids(&self) -> Result<Vec<String>, std::io::Error> {
        self.inner.repo_ids().await
    }

    async fn remove_repo(&self, repo_id: &str) -> Result<bool, std::io::Error> {
        self.inner.remove_repo(repo_id).await
    }

    fn invalidate_exists_cache(&self) {
        self.inner.invalidate_exists_cache();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use infra::storage::BlockStorageBackend;
    use migration::{Migrator, MigratorTrait};

    async fn repos() -> Arc<crate::repository::Repositories> {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        Migrator::up(&db, None).await.expect("migrate");
        Arc::new(crate::repository::Repositories::without_secrets(Arc::new(
            db,
        )))
    }

    /// A tracked write is announced before it happens; a deduplicated write drops the
    /// announcement instead of leaving a row for a block someone else owns.
    #[tokio::test]
    async fn tracked_writes_are_announced_and_dedupes_are_not_kept() {
        let repos = repos().await;
        let dir = tempfile::tempdir().unwrap();
        let raw: DynBlockStorage = infra::storage::new_block_store(dir.path());
        let store = PendingBlockStore::new(raw, repos.pending_block.clone());

        let data = b"one block";
        let id = infra::crypto::fs_id::sha1_hex(data);
        let (_, new) = store
            .write_block_with_id_tracked("repo", &id, data)
            .await
            .unwrap();
        assert!(new);
        assert!(
            repos
                .pending_block
                .written_after("repo", std::slice::from_ref(&id), 0)
                .await
                .unwrap()
                .contains(&id),
            "the write is announced"
        );

        let (_, new_again) = store
            .write_block_with_id_tracked("repo", &id, data)
            .await
            .unwrap();
        assert!(!new_again);
        assert!(
            repos
                .pending_block
                .written_after("repo", std::slice::from_ref(&id), 0)
                .await
                .unwrap()
                .is_empty(),
            "a deduplicated write must not keep the guard"
        );
    }

    /// Everything the wrapper is not interested in reaches the backend unchanged.
    #[tokio::test]
    async fn forwarding_keeps_the_backend_behaviour() {
        let repos = repos().await;
        let dir = tempfile::tempdir().unwrap();
        let raw: DynBlockStorage = infra::storage::new_block_store(dir.path());
        let store = PendingBlockStore::new(raw, repos.pending_block.clone());

        let id = store.write_block("repo", b"forwarded").await.unwrap();
        assert_eq!(store.read_block("repo", &id).await.unwrap(), b"forwarded");
        assert_eq!(store.block_size("repo", &id).await.unwrap(), 9);
        assert_eq!(store.list_blocks("repo").await.unwrap(), vec![id.clone()]);
        assert_eq!(store.repo_ids().await.unwrap(), vec!["repo".to_string()]);
        assert!(store.remove_repo("repo").await.unwrap());
        assert!(!store.has_block("repo", &id).await);
    }
}
