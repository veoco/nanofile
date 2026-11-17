use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, Statement,
};
use std::collections::HashSet;
use std::sync::Arc;

use base::error::AppError;
use infra::entity::pending_block;

/// SQLite's variable limit caps an `IN` list; chunk so a 128-block upload never trips it.
const BATCH: usize = 500;

/// The durable ledger of blocks an upload has written but no commit references yet.
///
/// One row per `(repo, block)` inserted *before* the block is written, so a crash
/// between the two leaves a row pointing at a block that may not exist — garbage
/// collection deletes a missing block idempotently, which is the safe direction. The
/// commit (and the abort) deletes the row; garbage collection expires whatever is
/// left after the grace window.
#[async_trait]
pub trait PendingBlockRepository: Send + Sync {
    /// Record the intent to write these blocks. Idempotent: a retried upload keeps
    /// the first timestamp, which only makes the block look older, never newer.
    async fn mark_written(
        &self,
        repo_id: &str,
        block_ids: &[String],
        written_at: i64,
    ) -> Result<(), AppError>;

    /// The blocks are now referenced by a commit, or were not written after all.
    async fn forget(&self, repo_id: &str, block_ids: &[String]) -> Result<(), AppError>;

    /// Subset of `block_ids` whose write is newer than `cutoff`.
    ///
    /// Garbage collection uses this instead of asking the store for each block: a
    /// hit means "written recently, leave it alone", and a miss means the block is
    /// older than the grace window.
    async fn written_after(
        &self,
        repo_id: &str,
        block_ids: &[String],
        cutoff: i64,
    ) -> Result<HashSet<String>, AppError>;

    /// Whether `repo_id` has any announcement written at or after `cutoff`.
    ///
    /// This is the durable half of the upload-activity guard: the in-process lease
    /// is empty in a separate `nanofile gc` process and after a restart, while the
    /// row survives both. A hit means the library may still be uploading, so a pass
    /// leaves its rows and blocks alone.
    async fn announced_since(&self, repo_id: &str, cutoff: i64) -> Result<bool, AppError>;

    /// Atomically remove up to `limit` rows written before `cutoff` and return them.
    ///
    /// Removing the row first is what makes the sweep safe against a commit racing
    /// it: a commit deletes the row too, so whichever runs second sees no row and
    /// does not delete the block.
    async fn claim_expired(
        &self,
        cutoff: i64,
        limit: usize,
    ) -> Result<Vec<(String, String)>, AppError>;

    /// The same selection without removing anything, for a dry run.
    async fn expired_before(
        &self,
        cutoff: i64,
        limit: usize,
    ) -> Result<Vec<(String, String)>, AppError>;
}

pub struct DbPendingBlockRepository {
    db: Arc<DatabaseConnection>,
}

impl DbPendingBlockRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl PendingBlockRepository for DbPendingBlockRepository {
    async fn mark_written(
        &self,
        repo_id: &str,
        block_ids: &[String],
        written_at: i64,
    ) -> Result<(), AppError> {
        for chunk in block_ids.chunks(BATCH) {
            let models: Vec<pending_block::ActiveModel> = chunk
                .iter()
                .map(|block_id| pending_block::ActiveModel {
                    repo_id: Set(repo_id.to_string()),
                    block_id: Set(block_id.clone()),
                    written_at: Set(written_at),
                })
                .collect();
            let result = pending_block::Entity::insert_many(models)
                .on_conflict(
                    sea_orm::sea_query::OnConflict::columns([
                        pending_block::Column::RepoId,
                        pending_block::Column::BlockId,
                    ])
                    .do_nothing()
                    .to_owned(),
                )
                .exec(self.db.as_ref())
                .await;
            // Every row conflicting is success here: a retried announcement.
            match result {
                Ok(_) | Err(DbErr::RecordNotInserted) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    async fn forget(&self, repo_id: &str, block_ids: &[String]) -> Result<(), AppError> {
        for chunk in block_ids.chunks(BATCH) {
            pending_block::Entity::delete_many()
                .filter(pending_block::Column::RepoId.eq(repo_id))
                .filter(pending_block::Column::BlockId.is_in(chunk.to_vec()))
                .exec(self.db.as_ref())
                .await?;
        }
        Ok(())
    }

    async fn written_after(
        &self,
        repo_id: &str,
        block_ids: &[String],
        cutoff: i64,
    ) -> Result<HashSet<String>, AppError> {
        let mut out = HashSet::new();
        for chunk in block_ids.chunks(BATCH) {
            let ids: Vec<(String,)> = pending_block::Entity::find()
                .select_only()
                .column(pending_block::Column::BlockId)
                .filter(pending_block::Column::RepoId.eq(repo_id))
                .filter(pending_block::Column::BlockId.is_in(chunk.to_vec()))
                .filter(pending_block::Column::WrittenAt.gte(cutoff))
                .into_tuple()
                .all(self.db.as_ref())
                .await?;
            out.extend(ids.into_iter().map(|(id,)| id));
        }
        Ok(out)
    }

    async fn announced_since(&self, repo_id: &str, cutoff: i64) -> Result<bool, AppError> {
        // Project one column and stop after the first row: this runs once per
        // library per pass, and only the existence of a recent write matters.
        let row: Option<(String,)> = pending_block::Entity::find()
            .select_only()
            .column(pending_block::Column::BlockId)
            .filter(pending_block::Column::RepoId.eq(repo_id))
            .filter(pending_block::Column::WrittenAt.gte(cutoff))
            .limit(1)
            .into_tuple()
            .one(self.db.as_ref())
            .await?;
        Ok(row.is_some())
    }

    async fn claim_expired(
        &self,
        cutoff: i64,
        limit: usize,
    ) -> Result<Vec<(String, String)>, AppError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        // SQLite has no `DELETE ... LIMIT` in a build without
        // SQLITE_ENABLE_UPDATE_DELETE_LIMIT, so the rowids are selected first. The
        // statement is atomic: a commit that removed its row in the meantime is
        // simply not returned, and its block is never touched.
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "DELETE FROM pending_blocks WHERE rowid IN ( \
                     SELECT rowid FROM pending_blocks WHERE written_at < ? \
                     ORDER BY written_at LIMIT ? \
                 ) RETURNING repo_id, block_id",
                [cutoff.into(), (limit as i64).into()],
            ))
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push((
                row.try_get::<String>("", "repo_id")?,
                row.try_get::<String>("", "block_id")?,
            ));
        }
        Ok(out)
    }

    async fn expired_before(
        &self,
        cutoff: i64,
        limit: usize,
    ) -> Result<Vec<(String, String)>, AppError> {
        let rows: Vec<(String, String)> = pending_block::Entity::find()
            .select_only()
            .column(pending_block::Column::RepoId)
            .column(pending_block::Column::BlockId)
            .filter(pending_block::Column::WrittenAt.lt(cutoff))
            .order_by_asc(pending_block::Column::WrittenAt)
            .limit(limit as u64)
            .into_tuple()
            .all(self.db.as_ref())
            .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use migration::{Migrator, MigratorTrait};

    async fn repo() -> DbPendingBlockRepository {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        Migrator::up(&db, None).await.expect("migrate");
        DbPendingBlockRepository::new(Arc::new(db))
    }

    /// Announcing is idempotent, the age lookup respects the cutoff, and claiming
    /// removes a row exactly once — that atomicity is what makes the sweep safe
    /// against a commit racing it.
    #[tokio::test]
    async fn announce_lookup_and_claim() {
        let repo = repo().await;
        let ids = vec!["a1".repeat(20), "b2".repeat(20)];
        repo.mark_written("r1", &ids, 100).await.unwrap();
        // A retried announcement keeps the first timestamp.
        repo.mark_written("r1", &ids, 900).await.unwrap();

        let fresh = repo.written_after("r1", &ids, 50).await.unwrap();
        assert_eq!(fresh.len(), 2);
        let none = repo.written_after("r1", &ids, 500).await.unwrap();
        assert!(none.is_empty(), "both were announced at 100");

        let claimed = repo.claim_expired(500, 10).await.unwrap();
        assert_eq!(claimed.len(), 2, "the claim returns the rows it removed");
        assert!(repo.claim_expired(500, 10).await.unwrap().is_empty());
        assert!(repo.written_after("r1", &ids, 0).await.unwrap().is_empty());
    }

    /// The durable activity probe sees only this repository's recent rows: an old
    /// announcement, or another library's fresh one, must not make it look busy.
    #[tokio::test]
    async fn announced_since_is_scoped_to_the_repository_and_the_cutoff() {
        let repo = repo().await;
        let old = "e5".repeat(20);
        let fresh = "f6".repeat(20);
        repo.mark_written("r1", std::slice::from_ref(&old), 100)
            .await
            .unwrap();
        repo.mark_written("r1", std::slice::from_ref(&fresh), 900)
            .await
            .unwrap();
        repo.mark_written("r2", std::slice::from_ref(&fresh), 900)
            .await
            .unwrap();

        assert!(repo.announced_since("r1", 500).await.unwrap());
        assert!(!repo.announced_since("r1", 901).await.unwrap());
        assert!(repo.announced_since("r2", 500).await.unwrap());
        assert!(!repo.announced_since("r3", 0).await.unwrap());
    }

    /// The two halves of the commit race: whoever removes the row second must not
    /// get it back.
    #[tokio::test]
    async fn forget_and_claim_cannot_both_win() {
        let repo = repo().await;
        let id = "c3".repeat(20);
        repo.mark_written("r1", std::slice::from_ref(&id), 100)
            .await
            .unwrap();
        repo.forget("r1", std::slice::from_ref(&id)).await.unwrap();
        assert!(
            repo.claim_expired(1_000, 10).await.unwrap().is_empty(),
            "a committed block is never claimed for deletion"
        );

        repo.mark_written("r1", std::slice::from_ref(&id), 100)
            .await
            .unwrap();
        assert_eq!(repo.claim_expired(1_000, 10).await.unwrap().len(), 1);
        // The commit's delete is a no-op now, which is the safe direction: the block
        // is already claimed, and the sweep re-checks references before deleting it.
        repo.forget("r1", std::slice::from_ref(&id)).await.unwrap();
    }

    /// A read-only preview does not consume the rows.
    #[tokio::test]
    async fn expired_before_does_not_remove() {
        let repo = repo().await;
        let id = "d4".repeat(20);
        repo.mark_written("r1", std::slice::from_ref(&id), 100)
            .await
            .unwrap();
        assert_eq!(repo.expired_before(500, 10).await.unwrap().len(), 1);
        assert_eq!(repo.expired_before(500, 10).await.unwrap().len(), 1);
    }
}
