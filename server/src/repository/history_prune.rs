use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::{commit, fs_object};

/// SQLite's variable limit caps one `IN` list; chunk the deletes.
const BATCH: usize = 500;

/// Deletes the database rows a history prune removes.
///
/// Garbage collection plans a set of commit rows and the file-object rows only they
/// referenced. The two sets belong to different tables but must disappear together:
/// deleting the objects first would leave commits pointing at missing trees if the
/// process died in between, which a later read could hit. Keeping the operation here,
/// next to the connection, is what makes one transaction possible without leaking
/// either entity into the pass.
#[async_trait]
pub trait HistoryPruneRepository: Send + Sync {
    /// Delete `commit_ids` and `fs_object_ids` atomically (commits first, so the
    /// referencing rows are gone before the objects they name).
    async fn prune(&self, commit_ids: Vec<i64>, fs_object_ids: Vec<i64>) -> Result<(), AppError>;
}

pub struct DbHistoryPruneRepository {
    db: Arc<DatabaseConnection>,
}

impl DbHistoryPruneRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl HistoryPruneRepository for DbHistoryPruneRepository {
    async fn prune(&self, commit_ids: Vec<i64>, fs_object_ids: Vec<i64>) -> Result<(), AppError> {
        if commit_ids.is_empty() && fs_object_ids.is_empty() {
            return Ok(());
        }
        let txn = self.db.begin().await?;
        for chunk in commit_ids.chunks(BATCH) {
            commit::Entity::delete_many()
                .filter(commit::Column::Id.is_in(chunk.to_vec()))
                .exec(&txn)
                .await?;
        }
        for chunk in fs_object_ids.chunks(BATCH) {
            fs_object::Entity::delete_many()
                .filter(fs_object::Column::Id.is_in(chunk.to_vec()))
                .exec(&txn)
                .await?;
        }
        txn.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use migration::{Migrator, MigratorTrait};
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};

    async fn repo() -> DbHistoryPruneRepository {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        Migrator::up(&db, None).await.expect("migrate");
        // The rows are seeded without their library and user parents; this test only
        // exercises the delete, and the constraints have nothing to say about it.
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "PRAGMA foreign_keys = OFF",
        ))
        .await
        .expect("disable foreign keys");
        DbHistoryPruneRepository::new(Arc::new(db))
    }

    async fn insert_commit(db: &DatabaseConnection, id: i64, repo_id: &str, commit_id: &str) {
        let root = "a".repeat(40);
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!(
                "INSERT INTO commits (id, repo_id, commit_id, root_id, creator_name, ctime) \
                 VALUES ({id}, '{repo_id}', '{commit_id}', '{root}', 'u', 0)"
            ),
        ))
        .await
        .expect("insert commit");
    }

    async fn insert_fs_object(db: &DatabaseConnection, id: i64, repo_id: &str, fs_id: &str) {
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!(
                "INSERT INTO fs_objects (id, repo_id, fs_id, obj_type, data) \
                 VALUES ({id}, '{repo_id}', '{fs_id}', 3, '{{}}')"
            ),
        ))
        .await
        .expect("insert fs object");
    }

    /// Only the named rows disappear, and both tables are done in one operation:
    /// other libraries and unnamed rows are untouched.
    #[tokio::test]
    async fn prune_removes_exactly_the_named_rows_in_both_tables() {
        let repo = repo().await;
        let db = repo.db.as_ref();
        let c1 = "c1".repeat(20);
        let c2 = "c2".repeat(20);
        let c3 = "c3".repeat(20);
        let f1 = "f1".repeat(20);
        let f2 = "f2".repeat(20);
        let f3 = "f3".repeat(20);
        insert_commit(db, 1, "r1", &c1).await;
        insert_commit(db, 2, "r1", &c2).await;
        insert_commit(db, 3, "r2", &c3).await;
        insert_fs_object(db, 10, "r1", &f1).await;
        insert_fs_object(db, 11, "r1", &f2).await;
        insert_fs_object(db, 12, "r2", &f3).await;

        repo.prune(vec![1], vec![10]).await.expect("prune");

        let remaining_commits: Vec<i64> = commit::Entity::find()
            .all(db)
            .await
            .expect("commits")
            .into_iter()
            .map(|row| row.id)
            .collect();
        let remaining_objects: Vec<i64> = fs_object::Entity::find()
            .all(db)
            .await
            .expect("objects")
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert_eq!(remaining_commits, vec![2, 3]);
        assert_eq!(remaining_objects, vec![11, 12]);

        // Nothing to do is a no-op, not an error.
        repo.prune(Vec::new(), Vec::new())
            .await
            .expect("empty prune");
    }
}
