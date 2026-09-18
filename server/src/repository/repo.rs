use async_trait::async_trait;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, FromQueryResult,
    QueryFilter, QueryOrder, QuerySelect, Set,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::repo;

/// Row shape for `SELECT SUM(size) AS total FROM repos WHERE owner_id = ?`.
/// `total` is `Option` because SQL `SUM` yields NULL when no rows match.
#[derive(FromQueryResult)]
struct TotalSize {
    total: Option<i64>,
}

/// Row shape for `SELECT owner_id, SUM(size) AS total FROM repos GROUP BY owner_id`.
#[derive(FromQueryResult)]
struct OwnerTotal {
    owner_id: i32,
    total: Option<i64>,
}

/// Parameters for creating a new repo.
pub struct CreateRepoParams {
    pub id: String,
    pub name: String,
    pub description: String,
    pub owner_id: i32,
    pub encrypted: i8,
    pub enc_version: i8,
    pub magic: Option<String>,
    pub random_key: Option<String>,
    pub salt: String,
    /// `pwd_hash` verifier triple of a Seafile 11+ encrypted library; all three
    /// are `None`/NULL for the legacy `magic` flow.
    pub pwd_hash: Option<String>,
    pub pwd_hash_algo: Option<String>,
    pub pwd_hash_params: Option<String>,
    pub permission: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Repository type (`"repo"` for a normal library).
    pub r#type: String,
}

/// The password-verifier columns rewritten by a password rotation.
///
/// `None` clears the column, which is what switching between the `magic` and
/// `pwd_hash` flows requires: a library carries one verifier or the other, never
/// both (upstream writes the `magic` commit field only when `!pwd_hash`).
#[derive(Default)]
pub struct UpdateRepoKeysParams {
    pub magic: Option<String>,
    pub random_key: Option<String>,
    pub pwd_hash: Option<String>,
    pub pwd_hash_algo: Option<String>,
    pub pwd_hash_params: Option<String>,
}

#[async_trait]
pub trait RepoRepository: Send + Sync {
    async fn find_by_id(&self, repo_id: &str) -> Result<Option<repo::Model>, AppError>;
    /// Fetch multiple repos by id in one query (chunked to stay under the
    /// SQLite variable limit). Repos that don't exist are simply absent.
    async fn find_by_ids(&self, repo_ids: &[String]) -> Result<Vec<repo::Model>, AppError>;
    async fn find_by_owner_id(&self, user_id: i32) -> Result<Vec<repo::Model>, AppError>;
    /// The earliest repo owned by the user (by created_at), used as the default
    /// library. Limits to a single row in SQL instead of loading every owned
    /// repo just to pick the first by creation time.
    async fn find_earliest_by_owner(&self, user_id: i32) -> Result<Option<repo::Model>, AppError>;
    /// Sum the `size` of all repos owned by a user, in a single SQL query.
    async fn sum_size_by_owner(&self, user_id: i32) -> Result<i64, AppError>;
    /// Sum the `size` of repos per owner, grouped in one query, for the given
    /// owner ids. Pairs with a total of 0 are omitted.
    async fn sum_sizes_by_owner(&self, user_ids: &[i32]) -> Result<Vec<(i32, i64)>, AppError>;
    /// Get all repos (used by garbage collection).
    async fn find_all(&self) -> Result<Vec<repo::Model>, AppError>;
    async fn create(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError>;
    /// Create a repo from typed parameters.
    async fn create_repo(&self, params: CreateRepoParams) -> Result<repo::Model, AppError>;
    async fn update(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError>;
    async fn update_head_commit(
        &self,
        repo_id: &str,
        head_commit_id: Option<String>,
    ) -> Result<(), AppError>;
    /// Compare-and-swap the HEAD commit: set `to` only if the stored value is
    /// still `from`. Returns whether the row matched.
    ///
    /// `update_head_commit` above is unconditional, which is only safe when the
    /// caller knows no other writer can be in flight. The sync branch update
    /// cannot assume that — two clients uploading at the same time both read
    /// HEAD, both compute a merge against it, and with an unconditional UPDATE
    /// the second write silently drops the first client's commit. Upstream does
    /// the equivalent with `seaf_branch_manager_test_and_update_branch()`
    /// (`common/branch-mgr.c`), which runs the comparison inside the same
    /// transaction as the write.
    ///
    /// This mirrors upstream's `SELECT ... FOR UPDATE` + conditional UPDATE
    /// inside one statement: SQLite serialises writes, and `rows_affected()`
    /// tells the caller whether it won the race.
    async fn test_and_update_head_commit(
        &self,
        repo_id: &str,
        from: Option<&str>,
        to: &str,
    ) -> Result<bool, AppError>;
    async fn delete_by_id(&self, repo_id: &str) -> Result<(), AppError>;
    /// Add a delta to the repo's size (can be negative).
    async fn adjust_size(&self, repo_id: &str, delta: i64) -> Result<(), AppError>;
    /// Update repo encryption keys (magic + random_key). Used by password change.
    async fn update_repo_keys(
        &self,
        repo_id: &str,
        params: UpdateRepoKeysParams,
    ) -> Result<(), AppError>;
    /// Rename a repo (owner-only).
    async fn rename_repo(&self, repo_id: &str, name: &str, updated_at: i64)
    -> Result<(), AppError>;
    /// Update repo name, description, and/or history retention settings
    /// (owner-only). `None` fields are left unchanged.
    async fn update_repo_details(
        &self,
        repo_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        history_limit: Option<i32>,
        history_ttl_days: Option<i32>,
        updated_at: i64,
    ) -> Result<(), AppError>;
}

pub struct DbRepoRepository {
    db: Arc<DatabaseConnection>,
}

impl DbRepoRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RepoRepository for DbRepoRepository {
    async fn create_repo(&self, params: CreateRepoParams) -> Result<repo::Model, AppError> {
        let model = repo::ActiveModel {
            id: Set(params.id.clone()),
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
        };
        // NOTE: `ActiveModelTrait::insert` (which uses `RETURNING *`) fails to
        // map the `type` column back to `r#type` for this entity, so insert and
        // re-fetch by id instead.
        repo::Entity::insert(model).exec(self.db.as_ref()).await?;
        self.find_by_id(&params.id)
            .await?
            .ok_or_else(|| AppError::Internal("Failed to find created repo".into()))
    }
    async fn find_by_id(&self, repo_id: &str) -> Result<Option<repo::Model>, AppError> {
        Ok(repo::Entity::find_by_id(repo_id)
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_ids(&self, repo_ids: &[String]) -> Result<Vec<repo::Model>, AppError> {
        if repo_ids.is_empty() {
            return Ok(Vec::new());
        }
        // SQLite has a ~999 bound on bound parameters; chunk the IN list.
        const IN_BATCH: usize = 500;
        let mut out = Vec::new();
        for chunk in repo_ids.chunks(IN_BATCH) {
            let rows = repo::Entity::find()
                .filter(repo::Column::Id.is_in(chunk))
                .all(self.db.as_ref())
                .await?;
            out.extend(rows);
        }
        Ok(out)
    }

    async fn find_by_owner_id(&self, user_id: i32) -> Result<Vec<repo::Model>, AppError> {
        Ok(repo::Entity::find()
            .filter(repo::Column::OwnerId.eq(user_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_earliest_by_owner(&self, user_id: i32) -> Result<Option<repo::Model>, AppError> {
        Ok(repo::Entity::find()
            .filter(repo::Column::OwnerId.eq(user_id))
            .order_by_asc(repo::Column::CreatedAt)
            .limit(1)
            .one(self.db.as_ref())
            .await?)
    }

    async fn sum_size_by_owner(&self, user_id: i32) -> Result<i64, AppError> {
        let row: Option<TotalSize> = repo::Entity::find()
            .filter(repo::Column::OwnerId.eq(user_id))
            .select_only()
            .column_as(Expr::col(repo::Column::Size).sum(), "total")
            .into_model()
            .one(self.db.as_ref())
            .await
            .map_err(|e| AppError::internal(format!("sum_size_by_owner: {e}")))?;
        Ok(row.and_then(|r| r.total).unwrap_or(0))
    }

    async fn sum_sizes_by_owner(&self, user_ids: &[i32]) -> Result<Vec<(i32, i64)>, AppError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<OwnerTotal> = repo::Entity::find()
            .filter(repo::Column::OwnerId.is_in(user_ids.to_vec()))
            .select_only()
            .column_as(repo::Column::OwnerId, "owner_id")
            .column_as(Expr::col(repo::Column::Size).sum(), "total")
            .group_by(repo::Column::OwnerId)
            .into_model()
            .all(self.db.as_ref())
            .await
            .map_err(|e| AppError::internal(format!("sum_sizes_by_owner: {e}")))?;
        Ok(rows
            .into_iter()
            .map(|r| (r.owner_id, r.total.unwrap_or(0)))
            .collect())
    }

    async fn find_all(&self) -> Result<Vec<repo::Model>, AppError> {
        Ok(repo::Entity::find().all(self.db.as_ref()).await?)
    }

    async fn create(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError> {
        // Extract the repo_id before insert (ActiveModel will be consumed)
        let repo_id = match &model.id {
            sea_orm::Set(id) => id.clone(),
            _ => return Err(AppError::Internal("repo id is required".into())),
        };
        // See `create_repo` — avoid `RETURNING` for this entity.
        repo::Entity::insert(model).exec(self.db.as_ref()).await?;
        self.find_by_id(&repo_id)
            .await?
            .ok_or_else(|| AppError::Internal("Failed to find created repo".into()))
    }

    async fn update(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError> {
        let result = model.update(self.db.as_ref()).await?;
        Ok(result)
    }

    async fn update_head_commit(
        &self,
        repo_id: &str,
        head_commit_id: Option<String>,
    ) -> Result<(), AppError> {
        repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .set(repo::ActiveModel {
                head_commit_id: Set(head_commit_id),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn test_and_update_head_commit(
        &self,
        repo_id: &str,
        from: Option<&str>,
        to: &str,
    ) -> Result<bool, AppError> {
        let mut query = repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .set(repo::ActiveModel {
                head_commit_id: Set(Some(to.to_string())),
                ..Default::default()
            });
        query = match from {
            Some(head) => query.filter(repo::Column::HeadCommitId.eq(head)),
            None => query.filter(repo::Column::HeadCommitId.is_null()),
        };
        let result = query.exec(self.db.as_ref()).await?;
        Ok(result.rows_affected == 1)
    }

    async fn delete_by_id(&self, repo_id: &str) -> Result<(), AppError> {
        repo::Entity::delete_by_id(repo_id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn adjust_size(&self, repo_id: &str, delta: i64) -> Result<(), AppError> {
        // Atomic `size = MAX(0, size + delta)` avoids the read-then-write lost
        // update that a separate `find_by_id` + `update_many` would introduce.
        repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .col_expr(
                repo::Column::Size,
                Expr::cust_with_values("MAX(0, size + ?)", [delta]),
            )
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn update_repo_keys(
        &self,
        repo_id: &str,
        params: UpdateRepoKeysParams,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .set(repo::ActiveModel {
                // `None` clears the column: rotating a `pwd_hash` library's
                // password must leave `magic` NULL, and vice versa.
                magic: Set(params.magic),
                random_key: Set(params.random_key),
                pwd_hash: Set(params.pwd_hash),
                pwd_hash_algo: Set(params.pwd_hash_algo),
                pwd_hash_params: Set(params.pwd_hash_params),
                updated_at: Set(now),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn rename_repo(
        &self,
        repo_id: &str,
        name: &str,
        updated_at: i64,
    ) -> Result<(), AppError> {
        repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .set(repo::ActiveModel {
                name: Set(name.to_string()),
                updated_at: Set(updated_at),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn update_repo_details(
        &self,
        repo_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        history_limit: Option<i32>,
        history_ttl_days: Option<i32>,
        updated_at: i64,
    ) -> Result<(), AppError> {
        let mut active: repo::ActiveModel = repo::ActiveModel {
            ..Default::default()
        };
        if let Some(n) = name {
            active.name = Set(n.to_string());
        }
        if let Some(d) = description {
            active.description = Set(d.to_string());
        }
        if let Some(hl) = history_limit {
            active.history_limit = Set(hl);
        }
        if let Some(ht) = history_ttl_days {
            active.history_ttl_days = Set(ht);
        }
        active.updated_at = Set(updated_at);

        repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .set(active)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use migration::MigratorTrait;
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};

    /// A migrated in-memory database with one user and two libraries: one with
    /// no HEAD (`repo-empty`) and one already at `h1`.
    async fn setup() -> DbRepoRepository {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        migration::Migrator::up(&db, None).await.unwrap();
        for sql in [
            "INSERT INTO users (id, email, password_hash, created_at) \
             VALUES (1, 'owner@example.com', 'x', 0)",
            "INSERT INTO repos (id, name, description, owner_id, encrypted, enc_version, salt, \
             permission, created_at, updated_at, size, repo_version, history_limit, \
             history_ttl_days) \
             VALUES ('repo-empty', 'empty', '', 1, 0, 0, '', 'rw', 0, 0, 0, 1, 0, 0)",
            "INSERT INTO repos (id, name, description, owner_id, encrypted, enc_version, salt, \
             permission, created_at, updated_at, size, repo_version, history_limit, \
             history_ttl_days, head_commit_id) \
             VALUES ('repo-h1', 'one', '', 1, 0, 0, '', 'rw', 0, 0, 0, 1, 0, 0, 'h1')",
        ] {
            db.execute_raw(Statement::from_string(DatabaseBackend::Sqlite, sql))
                .await
                .unwrap();
        }
        DbRepoRepository::new(std::sync::Arc::new(db))
    }

    async fn head(repo: &DbRepoRepository, repo_id: &str) -> Option<String> {
        repo.find_by_id(repo_id)
            .await
            .unwrap()
            .unwrap()
            .head_commit_id
    }

    /// A NULL HEAD only matches `from = None`, and wins.
    #[tokio::test]
    async fn test_and_update_matches_null_head() {
        let repo = setup().await;

        assert!(
            repo.test_and_update_head_commit("repo-empty", None, "c1")
                .await
                .unwrap()
        );
        assert_eq!(head(&repo, "repo-empty").await.as_deref(), Some("c1"));

        // The stored value is now `c1`, so the same NULL-based swap must miss.
        assert!(
            !repo
                .test_and_update_head_commit("repo-empty", None, "c2")
                .await
                .unwrap()
        );
        assert_eq!(head(&repo, "repo-empty").await.as_deref(), Some("c1"));
    }

    /// Only one of two writers racing on the same expected HEAD may win.
    #[tokio::test]
    async fn test_and_update_is_compare_and_swap() {
        let repo = setup().await;

        assert!(
            repo.test_and_update_head_commit("repo-h1", Some("h1"), "c1")
                .await
                .unwrap()
        );
        assert!(
            !repo
                .test_and_update_head_commit("repo-h1", Some("h1"), "c2")
                .await
                .unwrap(),
            "a stale expected HEAD must not overwrite the winner"
        );
        assert_eq!(head(&repo, "repo-h1").await.as_deref(), Some("c1"));

        // The winner's value is now the expected one.
        assert!(
            repo.test_and_update_head_commit("repo-h1", Some("c1"), "c3")
                .await
                .unwrap()
        );
        assert_eq!(head(&repo, "repo-h1").await.as_deref(), Some("c3"));

        // An unknown repo id matches no row.
        assert!(
            !repo
                .test_and_update_head_commit("nope", None, "c4")
                .await
                .unwrap()
        );
    }
}
