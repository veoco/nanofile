use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;

use base::error::AppError;
use infra::crypto::token_encryption::TokenCipher;
use infra::entity::sync_token;

use crate::domain::device::PeerStamp;

#[async_trait]
pub trait SyncTokenRepository: Send + Sync {
    /// The token one peer holds for a repository.
    ///
    /// `peer_id = None` addresses the *unattributed* token — the one minted by
    /// a caller with no device identity (a browser session, an API key, an
    /// older client). Every repository has at most one of those per user.
    async fn find_by_repo_user_peer(
        &self,
        repo_id: &str,
        user_id: i32,
        peer_id: Option<&str>,
    ) -> Result<Option<sync_token::Model>, AppError>;
    /// The same lookup for many repositories in one query (chunked to stay
    /// under the SQLite variable limit). Used to batch `accessible_repos` and
    /// `repo_tokens`.
    async fn find_by_repos_user_peer(
        &self,
        repo_ids: &[String],
        user_id: i32,
        peer_id: Option<&str>,
    ) -> Result<Vec<sync_token::Model>, AppError>;
    /// Look up a token row by its **raw** (client-presented) value.
    async fn find_by_token(&self, token: &str) -> Result<Option<sync_token::Model>, AppError>;
    async fn find_by_token_and_repo(
        &self,
        token: &str,
        repo_id: &str,
    ) -> Result<Option<sync_token::Model>, AppError>;
    /// Mint a token, recording the device it was issued to (if any) right away.
    ///
    /// A token that carries its peer from the start is visible under its device
    /// in the credential inventory without waiting for the first sync.
    async fn create(
        &self,
        repo_id: &str,
        user_id: i32,
        token: String,
        peer: Option<&PeerStamp>,
        now: i64,
        expires_at: Option<i64>,
    ) -> Result<(), AppError>;
    async fn delete_by_repo(&self, repo_id: &str) -> Result<(), AppError>;
    /// Delete by the **raw** token value.
    async fn delete_by_token(&self, token: &str) -> Result<(), AppError>;
    async fn delete_by_id(&self, id: i32) -> Result<(), AppError>;
    /// Every sync token a user holds, newest first, for the credential
    /// inventory.
    ///
    /// The stored value is a ciphertext of a bearer credential, so callers that
    /// only need to show *what exists* must not reveal it.
    async fn list_for_user(&self, user_id: i32) -> Result<Vec<sync_token::Model>, AppError>;
    /// Revoke one sync token, scoped to its owner: the plain `delete_by_id` is
    /// not safe to expose.
    async fn delete_by_id_and_user(&self, id: i32, user_id: i32) -> Result<u64, AppError>;
    async fn delete_by_user(&self, user_id: i32) -> Result<u64, AppError>;
    async fn delete_by_user_and_peer(&self, user_id: i32, peer_id: &str) -> Result<u64, AppError>;
    /// Delete a user's token for one repository.
    ///
    /// Called when a member loses access to a library: a sync token is bound to
    /// (repo, user) for up to `sync_token_ttl_days` (a year by default) and the
    /// sync endpoints re-derive the caller's identity from it, so it must not
    /// outlive the membership.
    async fn delete_by_repo_and_user(&self, repo_id: &str, user_id: i32) -> Result<u64, AppError>;
    /// Attribute an unattributed token to a device.
    ///
    /// Answers whether *this* call performed the update, so two devices racing
    /// for the same unattributed row cannot both believe they won: the loser
    /// mints its own token instead.
    async fn attach_peer_if_unset(&self, id: i32, peer: &PeerStamp) -> Result<bool, AppError>;
    /// Refresh the volatile peer columns from a sync request.
    ///
    /// Never touches `peer_id` or `peer_name`: a token's device is decided once
    /// (upstream's `RepoTokenPeerInfo` is insert-once too), so the device that
    /// happens to sync last cannot take another device's token over.
    async fn touch_peer(
        &self,
        id: i32,
        peer_ip: Option<String>,
        client_version: Option<String>,
        last_sync_time: i64,
    ) -> Result<(), AppError>;
    /// Recover the raw token from a stored row. `None` when the ciphertext
    /// cannot be decrypted (e.g. the server secret changed) — callers must
    /// treat that as an invalid credential.
    fn reveal_token(&self, model: &sync_token::Model) -> Option<String>;
}

pub struct DbSyncTokenRepository {
    db: Arc<DatabaseConnection>,
    cipher: Arc<TokenCipher>,
}

impl DbSyncTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>, cipher: Arc<TokenCipher>) -> Self {
        Self { db, cipher }
    }
}

#[async_trait]
impl SyncTokenRepository for DbSyncTokenRepository {
    async fn find_by_repo_user_peer(
        &self,
        repo_id: &str,
        user_id: i32,
        peer_id: Option<&str>,
    ) -> Result<Option<sync_token::Model>, AppError> {
        Ok(peer_query(
            sync_token::Entity::find()
                .filter(sync_token::Column::RepoId.eq(repo_id))
                .filter(sync_token::Column::UserId.eq(user_id)),
            peer_id,
        )
        .one(self.db.as_ref())
        .await?)
    }

    async fn find_by_repos_user_peer(
        &self,
        repo_ids: &[String],
        user_id: i32,
        peer_id: Option<&str>,
    ) -> Result<Vec<sync_token::Model>, AppError> {
        if repo_ids.is_empty() {
            return Ok(Vec::new());
        }
        // SQLite has a ~999 bound on bound parameters; chunk the IN list.
        const IN_BATCH: usize = 500;
        let mut out = Vec::new();
        for chunk in repo_ids.chunks(IN_BATCH) {
            let rows = peer_query(
                sync_token::Entity::find()
                    .filter(sync_token::Column::RepoId.is_in(chunk))
                    .filter(sync_token::Column::UserId.eq(user_id)),
                peer_id,
            )
            .all(self.db.as_ref())
            .await?;
            out.extend(rows);
        }
        Ok(out)
    }

    async fn find_by_token(&self, token: &str) -> Result<Option<sync_token::Model>, AppError> {
        // Deterministic AEAD: the stored value doubles as the lookup key, so a
        // client-presented raw token is encrypted before the equality query.
        let stored = self.cipher.encrypt(token);
        Ok(sync_token::Entity::find()
            .filter(sync_token::Column::Token.eq(stored))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_token_and_repo(
        &self,
        token: &str,
        repo_id: &str,
    ) -> Result<Option<sync_token::Model>, AppError> {
        let stored = self.cipher.encrypt(token);
        Ok(sync_token::Entity::find()
            .filter(sync_token::Column::Token.eq(stored))
            .filter(sync_token::Column::RepoId.eq(repo_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn create(
        &self,
        repo_id: &str,
        user_id: i32,
        token: String,
        peer: Option<&PeerStamp>,
        now: i64,
        expires_at: Option<i64>,
    ) -> Result<(), AppError> {
        sync_token::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            user_id: Set(user_id),
            token: Set(self.cipher.encrypt(&token)),
            peer_id: Set(peer.map(|p| p.id.clone())),
            peer_name: Set(peer.and_then(|p| p.name.clone())),
            peer_ip: Set(None),
            client_version: Set(peer.and_then(|p| p.client_version.clone())),
            created_at: Set(now),
            expires_at: Set(expires_at),
            last_sync_time: Set(None),
        }
        .insert(self.db.as_ref())
        .await?;
        Ok(())
    }

    async fn delete_by_repo(&self, repo_id: &str) -> Result<(), AppError> {
        sync_token::Entity::delete_many()
            .filter(sync_token::Column::RepoId.eq(repo_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_token(&self, token: &str) -> Result<(), AppError> {
        let stored = self.cipher.encrypt(token);
        sync_token::Entity::delete_many()
            .filter(sync_token::Column::Token.eq(stored))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_id(&self, id: i32) -> Result<(), AppError> {
        sync_token::Entity::delete_many()
            .filter(sync_token::Column::Id.eq(id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn list_for_user(&self, user_id: i32) -> Result<Vec<sync_token::Model>, AppError> {
        Ok(sync_token::Entity::find()
            .filter(sync_token::Column::UserId.eq(user_id))
            .order_by_desc(sync_token::Column::CreatedAt)
            .all(self.db.as_ref())
            .await?)
    }

    async fn delete_by_id_and_user(&self, id: i32, user_id: i32) -> Result<u64, AppError> {
        let result = sync_token::Entity::delete_many()
            .filter(sync_token::Column::Id.eq(id))
            .filter(sync_token::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn delete_by_user(&self, user_id: i32) -> Result<u64, AppError> {
        let result = sync_token::Entity::delete_many()
            .filter(sync_token::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn delete_by_repo_and_user(&self, repo_id: &str, user_id: i32) -> Result<u64, AppError> {
        let result = sync_token::Entity::delete_many()
            .filter(sync_token::Column::RepoId.eq(repo_id))
            .filter(sync_token::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn delete_by_user_and_peer(&self, user_id: i32, peer_id: &str) -> Result<u64, AppError> {
        let result = sync_token::Entity::delete_many()
            .filter(sync_token::Column::UserId.eq(user_id))
            .filter(sync_token::Column::PeerId.eq(peer_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn attach_peer_if_unset(&self, id: i32, peer: &PeerStamp) -> Result<bool, AppError> {
        // The `peer_id IS NULL` filter is the whole point: if another request
        // attached the row first, this UPDATE matches nothing and the caller
        // knows it lost the race instead of overwriting the winner.
        let result = sync_token::Entity::update_many()
            .filter(sync_token::Column::Id.eq(id))
            .filter(sync_token::Column::PeerId.is_null())
            .set(sync_token::ActiveModel {
                peer_id: Set(Some(peer.id.clone())),
                peer_name: Set(peer.name.clone()),
                client_version: Set(peer.client_version.clone()),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected > 0)
    }

    async fn touch_peer(
        &self,
        id: i32,
        peer_ip: Option<String>,
        client_version: Option<String>,
        last_sync_time: i64,
    ) -> Result<(), AppError> {
        let mut update = sync_token::ActiveModel {
            peer_ip: Set(peer_ip),
            last_sync_time: Set(Some(last_sync_time)),
            ..Default::default()
        };
        // A request that omits `client_ver` must not erase the version a
        // previous one recorded, so only write it when it was reported.
        if client_version.is_some() {
            update.client_version = Set(client_version);
        }
        sync_token::Entity::update_many()
            .filter(sync_token::Column::Id.eq(id))
            .set(update)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    fn reveal_token(&self, model: &sync_token::Model) -> Option<String> {
        self.cipher.decrypt(&model.token)
    }
}

/// Restrict a sync-token query to one peer, or to the unattributed row.
///
/// An empty `peer_id` is treated as absent everywhere: a caller with no device
/// and a caller whose device id is empty both mint the same unattributed row.
fn peer_query(
    query: sea_orm::Select<sync_token::Entity>,
    peer_id: Option<&str>,
) -> sea_orm::Select<sync_token::Entity> {
    match peer_id.filter(|peer| !peer.is_empty()) {
        Some(peer) => query.filter(sync_token::Column::PeerId.eq(peer)),
        None => query.filter(sync_token::Column::PeerId.is_null()),
    }
}

/// One-time migration: encrypt every legacy plaintext sync-token row in place.
///
/// Clients keep presenting the same raw token, so this is non-disruptive.
/// Rows that are already encrypted (prefix [`TOKEN_CIPHER_PREFIX`]) are skipped.
/// Returns the number of rows rewritten.
pub async fn encrypt_legacy_sync_tokens(
    db: &DatabaseConnection,
    cipher: &TokenCipher,
) -> Result<u64, AppError> {
    use infra::crypto::token_encryption::TOKEN_CIPHER_PREFIX;

    let legacy = sync_token::Entity::find()
        .filter(sync_token::Column::Token.not_like(format!("{TOKEN_CIPHER_PREFIX}%")))
        .all(db)
        .await?;

    let mut migrated = 0u64;
    for row in legacy {
        let stored = cipher.encrypt(&row.token);
        sync_token::Entity::update_many()
            .filter(sync_token::Column::Id.eq(row.id))
            .set(sync_token::ActiveModel {
                token: Set(stored),
                ..Default::default()
            })
            .exec(db)
            .await?;
        migrated += 1;
    }

    if migrated > 0 {
        tracing::info!(migrated, "encrypted legacy plaintext sync tokens at rest");
    }
    Ok(migrated)
}
