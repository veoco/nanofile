//! Provisioning a library is two rows that must exist together: the `repos` row
//! and the owner's `repo_members` row. Writing them through two repository calls
//! left a window where a failure produced a library its owner could not open, so
//! the pair is one transaction here. This follows the same shape as
//! [`history_prune`](crate::repository::history_prune): a narrow repository whose
//! job is one invariant.

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait, Set, TransactionTrait};

use base::error::AppError;
use infra::entity::{repo, repo_member};

use crate::repository::member::CreateMemberParams;
use crate::repository::repo::CreateRepoParams;

#[async_trait]
pub trait RepoProvisioningRepository: Send + Sync {
    /// Insert the library and its owner membership in one transaction. The
    /// returned model is the committed `repos` row.
    async fn create_repo_with_owner(
        &self,
        params: CreateRepoParams,
        member: CreateMemberParams,
    ) -> Result<repo::Model, AppError>;
}

pub struct DbRepoProvisioningRepository {
    db: Arc<DatabaseConnection>,
}

impl DbRepoProvisioningRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RepoProvisioningRepository for DbRepoProvisioningRepository {
    async fn create_repo_with_owner(
        &self,
        params: CreateRepoParams,
        member: CreateMemberParams,
    ) -> Result<repo::Model, AppError> {
        let repo_id = params.id.clone();
        let txn = self.db.begin().await?;
        repo::Entity::insert(repo::ActiveModel {
            id: Set(params.id),
            name: Set(params.name),
            description: Set(params.description),
            owner_id: Set(params.owner_id),
            encrypted: Set(params.encrypted),
            enc_version: Set(params.enc_version),
            magic: Set(params.magic),
            random_key: Set(params.random_key),
            salt: Set(params.salt),
            pwd_hash: Set(params.pwd_hash),
            pwd_hash_algo: Set(params.pwd_hash_algo),
            pwd_hash_params: Set(params.pwd_hash_params),
            head_commit_id: sea_orm::NotSet,
            permission: Set(params.permission),
            repo_version: Set(1),
            size: Set(0),
            created_at: Set(params.created_at),
            updated_at: Set(params.updated_at),
            history_limit: Set(0),
            history_ttl_days: Set(0),
            r#type: Set(params.r#type),
        })
        .exec(&txn)
        .await?;
        repo_member::Entity::insert(repo_member::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(member.repo_id),
            user_id: Set(member.user_id),
            permission: Set(member.permission),
            created_at: Set(member.created_at),
        })
        .exec(&txn)
        .await?;
        txn.commit().await?;
        repo::Entity::find_by_id(&repo_id)
            .one(self.db.as_ref())
            .await?
            .ok_or_else(|| AppError::internal("created repo could not be read back"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::repo::CreateRepoParams;
    use migration::{Migrator, MigratorTrait};
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, PaginatorTrait, Statement};

    async fn setup() -> DbRepoProvisioningRepository {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        Migrator::up(&db, None).await.expect("migrate");
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "INSERT INTO users (id, email, password_hash, created_at) \
             VALUES (1, 'owner@example.com', 'x', 0)",
        ))
        .await
        .expect("seed user");
        DbRepoProvisioningRepository::new(Arc::new(db))
    }

    fn repo_params() -> CreateRepoParams {
        CreateRepoParams {
            id: "repo-1".to_string(),
            name: "library".to_string(),
            description: String::new(),
            owner_id: 1,
            encrypted: 0,
            enc_version: 0,
            magic: None,
            random_key: None,
            salt: String::new(),
            pwd_hash: None,
            pwd_hash_algo: None,
            pwd_hash_params: None,
            permission: "rw".to_string(),
            created_at: 1,
            updated_at: 1,
            r#type: "repo".to_string(),
        }
    }

    fn member_params() -> CreateMemberParams {
        CreateMemberParams {
            repo_id: "repo-1".to_string(),
            user_id: 1,
            permission: "rw".to_string(),
            created_at: 1,
        }
    }

    #[tokio::test]
    async fn creates_the_library_and_its_owner_membership() {
        let repo = setup().await;
        let created = repo
            .create_repo_with_owner(repo_params(), member_params())
            .await
            .expect("provision");
        assert_eq!(created.id, "repo-1");
        let db = repo.db.as_ref();
        let members = repo_member::Entity::find()
            .count(db)
            .await
            .expect("count members");
        assert_eq!(members, 1);
    }

    /// A failed membership insert must take the library row with it: the whole
    /// point of the transaction is that the pair cannot exist half-written.
    #[tokio::test]
    async fn a_failed_member_insert_rolls_back_the_library() {
        let repo = setup().await;
        repo.db
            .execute_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "CREATE TRIGGER fail_member BEFORE INSERT ON repo_members \
                 BEGIN SELECT RAISE(ABORT, 'boom'); END",
            ))
            .await
            .expect("create trigger");
        let result = repo
            .create_repo_with_owner(repo_params(), member_params())
            .await;
        assert!(result.is_err(), "the trigger must fail the transaction");
        let rows = repo::Entity::find()
            .count(repo.db.as_ref())
            .await
            .expect("count repos");
        assert_eq!(rows, 0, "the library row must have been rolled back");
    }
}
