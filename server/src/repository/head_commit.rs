//! Commit + HEAD in one transaction.
//!
//! A tree update inserts the new `commits` row and compare-and-swaps `repos.head_commit_id`.
//! Done as two independent writes, a lost CAS left an unreferenced commit row behind;
//! done in one transaction, the conflict rolls the insert back with it.

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait, TransactionTrait};

use base::error::AppError;
use infra::entity::commit;

use crate::repository::repo::advance_head_and_size_in;

#[async_trait]
pub trait HeadCommitRepository: Send + Sync {
    /// Insert `commit` and move HEAD from `from` to its id with `size_delta`, in
    /// one transaction. Returns `false` (with the insert rolled back) when HEAD no
    /// longer points at `from`.
    async fn insert_and_advance(
        &self,
        commit: commit::ActiveModel,
        repo_id: &str,
        from: Option<&str>,
        updated_at: i64,
        size_delta: i64,
    ) -> Result<bool, AppError>;
}

pub struct DbHeadCommitRepository {
    db: Arc<DatabaseConnection>,
}

impl DbHeadCommitRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl HeadCommitRepository for DbHeadCommitRepository {
    async fn insert_and_advance(
        &self,
        commit: commit::ActiveModel,
        repo_id: &str,
        from: Option<&str>,
        updated_at: i64,
        size_delta: i64,
    ) -> Result<bool, AppError> {
        let commit_id = match &commit.commit_id {
            sea_orm::ActiveValue::Set(id) => id.clone(),
            _ => return Err(AppError::internal("commit model has no commit id")),
        };
        let txn = self.db.begin().await?;
        commit::Entity::insert(commit).exec(&txn).await?;
        if !advance_head_and_size_in(&txn, repo_id, from, &commit_id, updated_at, size_delta)
            .await?
        {
            txn.rollback().await?;
            return Ok(false);
        }
        txn.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::repo::RepoRepository;
    use migration::{Migrator, MigratorTrait};
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, PaginatorTrait, Set, Statement};

    async fn setup() -> DbHeadCommitRepository {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        Migrator::up(&db, None).await.expect("migrate");
        for sql in [
            "INSERT INTO users (id, email, password_hash, created_at) \
             VALUES (1, 'owner@example.com', 'x', 0)",
            "INSERT INTO repos (id, name, description, owner_id, encrypted, enc_version, salt, \
             permission, created_at, updated_at, size, repo_version, history_limit, \
             history_ttl_days, head_commit_id) \
             VALUES ('repo-1', 'library', '', 1, 0, 0, '', 'rw', 0, 0, 10, 1, 0, 0, 'head-0')",
        ] {
            db.execute_raw(Statement::from_string(DatabaseBackend::Sqlite, sql))
                .await
                .expect("seed");
        }
        DbHeadCommitRepository::new(Arc::new(db))
    }

    fn commit_model(commit_id: &str) -> commit::ActiveModel {
        commit::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set("repo-1".to_string()),
            commit_id: Set(commit_id.to_string()),
            root_id: Set("a".repeat(40)),
            parent_id: Set(Some("head-0".to_string())),
            second_parent_id: sea_orm::NotSet,
            creator_name: Set("owner@example.com".to_string()),
            creator: Set(String::new()),
            description: Set("commit".to_string()),
            ctime: Set(1),
            version: Set(1),
            new_merge: sea_orm::NotSet,
            conflict: sea_orm::NotSet,
        }
    }

    #[tokio::test]
    async fn a_won_cas_commits_and_moves_head() {
        let repo = setup().await;
        let won = repo
            .insert_and_advance(commit_model("c-new"), "repo-1", Some("head-0"), 2, 5)
            .await
            .expect("insert and advance");
        assert!(won);
        let row = crate::repository::repo::DbRepoRepository::new(repo.db.clone())
            .find_by_id("repo-1")
            .await
            .expect("read repo")
            .expect("repo exists");
        assert_eq!(row.head_commit_id.as_deref(), Some("c-new"));
        assert_eq!(row.size, 15);
        let commits = commit::Entity::find()
            .count(repo.db.as_ref())
            .await
            .expect("count commits");
        assert_eq!(commits, 1);
    }

    #[tokio::test]
    async fn a_lost_cas_rolls_back_the_commit_row() {
        let repo = setup().await;
        let won = repo
            .insert_and_advance(commit_model("c-lost"), "repo-1", Some("other-head"), 2, 5)
            .await
            .expect("insert and advance");
        assert!(!won, "the stale head must lose the compare-and-swap");
        let commits = commit::Entity::find()
            .count(repo.db.as_ref())
            .await
            .expect("count commits");
        assert_eq!(commits, 0, "the orphan commit row must be rolled back");
    }
}
