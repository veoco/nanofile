//! Data access for unified API keys.
//!
//! A key is looked up by the hash of the presented secret. The auth path also
//! needs the key's library bindings, so [`ApiKeyRepository::find_by_presented`]
//! returns both in one step and the caching decorator can store the pair.

use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use std::collections::HashSet;
use std::sync::Arc;

use base::error::AppError;
use infra::entity::{api_key, api_key_repo};

/// SQLite binds at most ~999 parameters per statement.
const IN_BATCH: usize = 500;

/// One library binding to persist alongside a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiKeyBinding {
    pub repo_id: String,
    /// Read/write ceiling for this library: `"r"` or `"rw"`.
    pub permission: String,
}

/// A key together with its library bindings.
#[derive(Clone, Debug)]
pub struct ApiKeyLookup {
    pub key: api_key::Model,
    pub bindings: Vec<api_key_repo::Model>,
}

/// Parameters for creating a key.
pub struct CreateApiKeyParams {
    pub user_id: i32,
    pub name: String,
    /// `hex(sha256(raw))`; the plaintext is never persisted.
    pub key_hash: String,
    /// Leading characters of the plaintext, for display only.
    pub key_prefix: String,
    /// Canonical comma-separated capability list.
    pub capabilities: String,
    pub all_repos: bool,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub bindings: Vec<ApiKeyBinding>,
}

/// Fields an update may change.
///
/// `None` leaves a field untouched. For `expires_at` the outer `Option`
/// distinguishes "leave it" from the inner `None`, which means "never expires".
#[derive(Default)]
pub struct UpdateApiKeyParams {
    pub name: Option<String>,
    pub capabilities: Option<String>,
    pub all_repos: Option<bool>,
    pub expires_at: Option<Option<i64>>,
}

#[async_trait]
pub trait ApiKeyRepository: Send + Sync {
    /// Resolve a presented secret, returning the key and its bindings.
    async fn find_by_presented(&self, raw: &str) -> Result<Option<ApiKeyLookup>, AppError>;
    async fn find_by_id_and_user(
        &self,
        key_id: i32,
        user_id: i32,
    ) -> Result<Option<api_key::Model>, AppError>;
    /// Load a key regardless of owner (repo owners and admins manage keys they
    /// did not create).
    async fn find_by_id(&self, key_id: i32) -> Result<Option<api_key::Model>, AppError>;
    async fn find_by_user(&self, user_id: i32) -> Result<Vec<api_key::Model>, AppError>;
    async fn create(&self, params: CreateApiKeyParams) -> Result<api_key::Model, AppError>;
    /// Apply metadata changes. Returns whether a row matched.
    async fn update_metadata(
        &self,
        key_id: i32,
        user_id: i32,
        params: UpdateApiKeyParams,
    ) -> Result<bool, AppError>;
    /// Revoke a key. Returns whether a row was deleted.
    async fn delete_by_id_and_user(&self, key_id: i32, user_id: i32) -> Result<bool, AppError>;
    /// Revoke a key regardless of owner. Callers check authorization first.
    async fn delete_by_id(&self, key_id: i32) -> Result<bool, AppError>;
    /// Revoke every key a user holds (password change / reset).
    async fn delete_by_user(&self, user_id: i32) -> Result<u64, AppError>;
    async fn list_bindings(&self, key_id: i32) -> Result<Vec<api_key_repo::Model>, AppError>;
    async fn list_bindings_for_keys(
        &self,
        key_ids: &[i32],
    ) -> Result<Vec<api_key_repo::Model>, AppError>;
    /// Replace a key's library bindings atomically.
    async fn replace_bindings(
        &self,
        key_id: i32,
        bindings: &[ApiKeyBinding],
    ) -> Result<(), AppError>;
    /// Drop every binding that points at a deleted library, then delete the
    /// now-unbound keys. Returns the number of bindings removed.
    async fn delete_bindings_by_repo(&self, repo_id: &str) -> Result<u64, AppError>;
    /// Drop one user's bindings for a library and delete any key left without
    /// bindings. Returns the number of bindings removed.
    ///
    /// Not part of the current revocation policy: removing a member keeps their
    /// bindings, because every request re-checks membership (so the key is inert
    /// while they are out) and re-adding them legitimately restores the key they
    /// already hold. Available for an explicit purge.
    async fn delete_bindings_for_repo_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<u64, AppError>;
    /// Delete keys that are not `all_repos` and have no bindings left.
    async fn delete_orphan_bound_keys(&self) -> Result<u64, AppError>;
    /// Record last use. Called on the hot path, so it must not invalidate the
    /// auth cache (the cached model's `last_used_at` is never read for auth).
    async fn touch_last_used(&self, key_id: i32, ts: i64) -> Result<(), AppError>;
}

pub struct DbApiKeyRepository {
    db: Arc<DatabaseConnection>,
}

impl DbApiKeyRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    async fn insert_bindings(
        conn: &impl ConnectionTrait,
        key_id: i32,
        bindings: &[ApiKeyBinding],
    ) -> Result<(), AppError> {
        for binding in bindings {
            api_key_repo::ActiveModel {
                id: sea_orm::NotSet,
                key_id: Set(key_id),
                repo_id: Set(binding.repo_id.clone()),
                permission: Set(binding.permission.clone()),
            }
            .insert(conn)
            .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl ApiKeyRepository for DbApiKeyRepository {
    async fn find_by_presented(&self, raw: &str) -> Result<Option<ApiKeyLookup>, AppError> {
        let key_hash = crate::service::auth::token::hash_token(raw);
        let Some(key) = api_key::Entity::find()
            .filter(api_key::Column::KeyHash.eq(key_hash))
            .one(self.db.as_ref())
            .await?
        else {
            return Ok(None);
        };
        let bindings = self.list_bindings(key.id).await?;
        Ok(Some(ApiKeyLookup { key, bindings }))
    }

    async fn find_by_id_and_user(
        &self,
        key_id: i32,
        user_id: i32,
    ) -> Result<Option<api_key::Model>, AppError> {
        Ok(api_key::Entity::find()
            .filter(api_key::Column::Id.eq(key_id))
            .filter(api_key::Column::UserId.eq(user_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_id(&self, key_id: i32) -> Result<Option<api_key::Model>, AppError> {
        Ok(api_key::Entity::find()
            .filter(api_key::Column::Id.eq(key_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_user(&self, user_id: i32) -> Result<Vec<api_key::Model>, AppError> {
        Ok(api_key::Entity::find()
            .filter(api_key::Column::UserId.eq(user_id))
            .order_by_desc(api_key::Column::CreatedAt)
            .all(self.db.as_ref())
            .await?)
    }

    async fn create(&self, params: CreateApiKeyParams) -> Result<api_key::Model, AppError> {
        let txn = self.db.begin().await?;
        let model = api_key::ActiveModel {
            id: sea_orm::NotSet,
            user_id: Set(params.user_id),
            name: Set(params.name),
            key_hash: Set(params.key_hash),
            key_prefix: Set(Some(params.key_prefix)),
            capabilities: Set(params.capabilities),
            all_repos: Set(params.all_repos),
            created_at: Set(params.created_at),
            expires_at: Set(params.expires_at),
            last_used_at: Set(None),
        }
        .insert(&txn)
        .await?;
        // A key that is not `all_repos` without bindings would be inert; the
        // service validates that, and the transaction keeps the pair consistent
        // if the binding insert fails.
        Self::insert_bindings(&txn, model.id, &params.bindings).await?;
        txn.commit().await?;
        Ok(model)
    }

    async fn update_metadata(
        &self,
        key_id: i32,
        user_id: i32,
        params: UpdateApiKeyParams,
    ) -> Result<bool, AppError> {
        let mut active = api_key::ActiveModel {
            ..std::default::Default::default()
        };
        let mut any_change = false;
        if let Some(name) = params.name {
            active.name = Set(name);
            any_change = true;
        }
        if let Some(capabilities) = params.capabilities {
            active.capabilities = Set(capabilities);
            any_change = true;
        }
        if let Some(all_repos) = params.all_repos {
            active.all_repos = Set(all_repos);
            any_change = true;
        }
        if let Some(expires_at) = params.expires_at {
            active.expires_at = Set(expires_at);
            any_change = true;
        }
        // `update_many` with no SET values would emit invalid SQL; an update
        // that changes nothing is reported as a match instead.
        if !any_change {
            return Ok(self.find_by_id_and_user(key_id, user_id).await?.is_some());
        }
        let result = api_key::Entity::update_many()
            .set(active)
            .filter(api_key::Column::Id.eq(key_id))
            .filter(api_key::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected > 0)
    }

    async fn delete_by_id_and_user(&self, key_id: i32, user_id: i32) -> Result<bool, AppError> {
        let result = api_key::Entity::delete_many()
            .filter(api_key::Column::Id.eq(key_id))
            .filter(api_key::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected > 0)
    }

    async fn delete_by_id(&self, key_id: i32) -> Result<bool, AppError> {
        let result = api_key::Entity::delete_many()
            .filter(api_key::Column::Id.eq(key_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected > 0)
    }

    async fn delete_by_user(&self, user_id: i32) -> Result<u64, AppError> {
        let result = api_key::Entity::delete_many()
            .filter(api_key::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn list_bindings(&self, key_id: i32) -> Result<Vec<api_key_repo::Model>, AppError> {
        Ok(api_key_repo::Entity::find()
            .filter(api_key_repo::Column::KeyId.eq(key_id))
            .order_by_asc(api_key_repo::Column::RepoId)
            .all(self.db.as_ref())
            .await?)
    }

    async fn list_bindings_for_keys(
        &self,
        key_ids: &[i32],
    ) -> Result<Vec<api_key_repo::Model>, AppError> {
        if key_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for chunk in key_ids.chunks(IN_BATCH) {
            out.extend(
                api_key_repo::Entity::find()
                    .filter(api_key_repo::Column::KeyId.is_in(chunk.to_vec()))
                    .order_by_asc(api_key_repo::Column::RepoId)
                    .all(self.db.as_ref())
                    .await?,
            );
        }
        Ok(out)
    }

    async fn replace_bindings(
        &self,
        key_id: i32,
        bindings: &[ApiKeyBinding],
    ) -> Result<(), AppError> {
        let txn = self.db.begin().await?;
        api_key_repo::Entity::delete_many()
            .filter(api_key_repo::Column::KeyId.eq(key_id))
            .exec(&txn)
            .await?;
        Self::insert_bindings(&txn, key_id, bindings).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn delete_bindings_by_repo(&self, repo_id: &str) -> Result<u64, AppError> {
        let result = api_key_repo::Entity::delete_many()
            .filter(api_key_repo::Column::RepoId.eq(repo_id))
            .exec(self.db.as_ref())
            .await?;
        self.delete_orphan_bound_keys().await?;
        Ok(result.rows_affected)
    }

    async fn delete_bindings_for_repo_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<u64, AppError> {
        let key_ids: Vec<i32> = api_key::Entity::find()
            .filter(api_key::Column::UserId.eq(user_id))
            .all(self.db.as_ref())
            .await?
            .into_iter()
            .map(|key| key.id)
            .collect();
        if key_ids.is_empty() {
            return Ok(0);
        }
        let result = api_key_repo::Entity::delete_many()
            .filter(api_key_repo::Column::RepoId.eq(repo_id))
            .filter(api_key_repo::Column::KeyId.is_in(key_ids))
            .exec(self.db.as_ref())
            .await?;
        self.delete_orphan_bound_keys().await?;
        Ok(result.rows_affected)
    }

    async fn delete_orphan_bound_keys(&self) -> Result<u64, AppError> {
        let bound: HashSet<i32> = api_key_repo::Entity::find()
            .all(self.db.as_ref())
            .await?
            .into_iter()
            .map(|binding| binding.key_id)
            .collect();
        let orphans: Vec<i32> = api_key::Entity::find()
            .filter(api_key::Column::AllRepos.eq(false))
            .all(self.db.as_ref())
            .await?
            .into_iter()
            .map(|key| key.id)
            .filter(|id| !bound.contains(id))
            .collect();
        if orphans.is_empty() {
            return Ok(0);
        }
        let result = api_key::Entity::delete_many()
            .filter(api_key::Column::Id.is_in(orphans))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn touch_last_used(&self, key_id: i32, ts: i64) -> Result<(), AppError> {
        api_key::Entity::update_many()
            .filter(api_key::Column::Id.eq(key_id))
            .set(api_key::ActiveModel {
                last_used_at: Set(Some(ts)),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
