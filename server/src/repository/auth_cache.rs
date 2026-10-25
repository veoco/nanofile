//! In-memory TTL cache decorators for token lookups.
//!
//! Each authenticated sync request would otherwise pay a DB lookup plus SQLite lock wait; these
//! decorators cache `find_by_token` by raw token so the hot path short-circuits to memory.
//!
//! Correctness and security: the TTL (5 min) bounds staleness and an entry past its `expires_at`
//! is evicted on read; any `create`/`delete*` clears the whole cache so a regenerated or revoked
//! token takes effect immediately; `attach_peer_if_unset`/`touch_peer` deliberately do not clear
//! it, because the auth layer never reads peer fields from the cached model and the conditional
//! UPDATE decides attribution; and the auth layer still checks user existence plus `is_active`
//! against the DB on every request, so a stale token cannot authenticate a deleted account.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use base::error::AppError;
use infra::entity::{api_key, api_token, sync_token};

use crate::domain::device::PeerStamp;

use super::api_key::{
    ApiKeyBinding, ApiKeyLookup, ApiKeyRepository, CreateApiKeyParams, UpdateApiKeyParams,
};
use super::api_token::{ApiTokenRepository, CreateSessionTokenParams};
use super::sync_token::SyncTokenRepository;

const TOKEN_CACHE_TTL: Duration = Duration::from_secs(300);

/// Hard cap on cached tokens. A token is keyed by its raw value and most clients
/// reuse one per device, so the working set is small; the cap only matters when a
/// caller mints many short-lived tokens. Hitting it drops expired entries first,
/// then the whole cache, which is safe because every miss falls back to the DB.
const TOKEN_CACHE_MAX_ENTRIES: usize = 10_000;

struct CachedEntry<T> {
    value: T,
    cached_at: Instant,
}

struct TokenCache<T> {
    entries: Mutex<HashMap<String, CachedEntry<T>>>,
    ttl: Duration,
}

impl<T: Clone> TokenCache<T> {
    fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Fresh cached value for `key`, or `None`; a value failing `is_valid` is evicted.
    fn get(&self, key: &str, is_valid: impl Fn(&T) -> bool) -> Option<T> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries.get(key)?;
        if entry.cached_at.elapsed() > self.ttl || !is_valid(&entry.value) {
            entries.remove(key);
            return None;
        }
        Some(entry.value.clone())
    }

    fn insert(&self, key: &str, value: T) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= TOKEN_CACHE_MAX_ENTRIES {
            entries.retain(|_, entry| entry.cached_at.elapsed() <= self.ttl);
            if entries.len() >= TOKEN_CACHE_MAX_ENTRIES {
                entries.clear();
            }
        }
        entries.insert(
            key.to_string(),
            CachedEntry {
                value,
                cached_at: Instant::now(),
            },
        );
    }

    fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }
}

/// A token has expired when its `expires_at` is set and in the past.
fn token_expired(expires_at: Option<i64>) -> bool {
    matches!(expires_at, Some(exp) if chrono::Utc::now().timestamp() > exp)
}

pub struct CachingSyncTokenRepository {
    inner: Arc<dyn SyncTokenRepository>,
    cache: TokenCache<sync_token::Model>,
}

impl CachingSyncTokenRepository {
    pub fn new(inner: Arc<dyn SyncTokenRepository>) -> Self {
        Self {
            inner,
            cache: TokenCache::new(TOKEN_CACHE_TTL),
        }
    }
}

#[async_trait]
impl SyncTokenRepository for CachingSyncTokenRepository {
    async fn find_by_repo_user_peer(
        &self,
        repo_id: &str,
        user_id: i32,
        peer_id: Option<&str>,
    ) -> Result<Option<sync_token::Model>, AppError> {
        self.inner
            .find_by_repo_user_peer(repo_id, user_id, peer_id)
            .await
    }

    async fn find_by_repos_user_peer(
        &self,
        repo_ids: &[String],
        user_id: i32,
        peer_id: Option<&str>,
    ) -> Result<Vec<sync_token::Model>, AppError> {
        self.inner
            .find_by_repos_user_peer(repo_ids, user_id, peer_id)
            .await
    }

    async fn find_by_token(&self, token: &str) -> Result<Option<sync_token::Model>, AppError> {
        if let Some(model) = self.cache.get(token, |m| !token_expired(m.expires_at)) {
            return Ok(Some(model));
        }
        let result = self.inner.find_by_token(token).await?;
        if let Some(model) = &result {
            self.cache.insert(token, model.clone());
        }
        Ok(result)
    }

    async fn find_by_token_and_repo(
        &self,
        token: &str,
        repo_id: &str,
    ) -> Result<Option<sync_token::Model>, AppError> {
        self.inner.find_by_token_and_repo(token, repo_id).await
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
        let result = self
            .inner
            .create(repo_id, user_id, token, peer, now, expires_at)
            .await;
        self.cache.clear();
        result
    }

    async fn delete_by_repo(&self, repo_id: &str) -> Result<(), AppError> {
        let result = self.inner.delete_by_repo(repo_id).await;
        self.cache.clear();
        result
    }

    async fn delete_by_token(&self, token: &str) -> Result<(), AppError> {
        let result = self.inner.delete_by_token(token).await;
        self.cache.clear();
        result
    }

    async fn delete_by_id(&self, id: i32) -> Result<(), AppError> {
        let result = self.inner.delete_by_id(id).await;
        self.cache.clear();
        result
    }

    async fn list_for_user(&self, user_id: i32) -> Result<Vec<sync_token::Model>, AppError> {
        // Not cached: the cache holds per-credential lookups rather than lists.
        self.inner.list_for_user(user_id).await
    }

    async fn delete_by_id_and_user(&self, id: i32, user_id: i32) -> Result<u64, AppError> {
        let result = self.inner.delete_by_id_and_user(id, user_id).await;
        // Clearing now makes the revocation bite immediately rather than after the cache TTL.
        self.cache.clear();
        result
    }

    async fn delete_by_user(&self, user_id: i32) -> Result<u64, AppError> {
        let result = self.inner.delete_by_user(user_id).await;
        self.cache.clear();
        result
    }

    async fn delete_by_repo_and_user(&self, repo_id: &str, user_id: i32) -> Result<u64, AppError> {
        let result = self.inner.delete_by_repo_and_user(repo_id, user_id).await;
        self.cache.clear();
        result
    }

    async fn delete_by_user_and_peer(&self, user_id: i32, peer_id: &str) -> Result<u64, AppError> {
        let result = self.inner.delete_by_user_and_peer(user_id, peer_id).await;
        self.cache.clear();
        result
    }

    async fn attach_peer_if_unset(&self, id: i32, peer: &PeerStamp) -> Result<bool, AppError> {
        // Deliberately does not clear the cache: peer info is not consulted by authentication
        // and this runs on the first sync of every token. The conditional UPDATE decides who wins.
        self.inner.attach_peer_if_unset(id, peer).await
    }

    async fn touch_peer(
        &self,
        id: i32,
        peer_ip: Option<String>,
        client_version: Option<String>,
        last_sync_time: i64,
    ) -> Result<(), AppError> {
        // Same reason as `attach_peer_if_unset`: peer columns do not affect authorization.
        self.inner
            .touch_peer(id, peer_ip, client_version, last_sync_time)
            .await
    }

    fn reveal_token(&self, model: &sync_token::Model) -> Option<String> {
        self.inner.reveal_token(model)
    }
}

pub struct CachingApiTokenRepository {
    inner: Arc<dyn ApiTokenRepository>,
    cache: TokenCache<api_token::Model>,
}

impl CachingApiTokenRepository {
    pub fn new(inner: Arc<dyn ApiTokenRepository>) -> Self {
        Self {
            inner,
            cache: TokenCache::new(TOKEN_CACHE_TTL),
        }
    }
}

#[async_trait]
impl ApiTokenRepository for CachingApiTokenRepository {
    async fn find_by_token(&self, token: &str) -> Result<Option<api_token::Model>, AppError> {
        if let Some(model) = self.cache.get(token, |m| !token_expired(m.expires_at)) {
            return Ok(Some(model));
        }
        let result = self.inner.find_by_token(token).await?;
        if let Some(model) = &result {
            self.cache.insert(token, model.clone());
        }
        Ok(result)
    }

    async fn delete_many_by_device(&self, device_id: &str) -> Result<(), AppError> {
        let result = self.inner.delete_many_by_device(device_id).await;
        self.cache.clear();
        result
    }

    async fn delete_many_by_user_platform_device(
        &self,
        user_id: i32,
        platform: &str,
        device_id: &str,
    ) -> Result<u64, AppError> {
        let result = self
            .inner
            .delete_many_by_user_platform_device(user_id, platform, device_id)
            .await;
        self.cache.clear();
        result
    }

    async fn delete_many_by_user_and_device(
        &self,
        user_id: i32,
        device_id: &str,
    ) -> Result<u64, AppError> {
        let result = self
            .inner
            .delete_many_by_user_and_device(user_id, device_id)
            .await;
        self.cache.clear();
        result
    }

    async fn delete_by_token(&self, token: &str) -> Result<(), AppError> {
        let result = self.inner.delete_by_token(token).await;
        self.cache.clear();
        result
    }

    async fn insert(&self, model: api_token::ActiveModel) -> Result<(), AppError> {
        let result = self.inner.insert(model).await;
        self.cache.clear();
        result
    }

    async fn delete_many_by_user_id(&self, user_id: i32) -> Result<(), AppError> {
        let result = self.inner.delete_many_by_user_id(user_id).await;
        self.cache.clear();
        result
    }

    async fn delete_many_by_user_id_except(
        &self,
        user_id: i32,
        keep_raw_token: &str,
    ) -> Result<(), AppError> {
        let result = self
            .inner
            .delete_many_by_user_id_except(user_id, keep_raw_token)
            .await;
        self.cache.clear();
        result
    }

    async fn delete_browser_sessions_except(
        &self,
        user_id: i32,
        keep_id: Option<i32>,
    ) -> Result<u64, AppError> {
        let result = self
            .inner
            .delete_browser_sessions_except(user_id, keep_id)
            .await;
        self.cache.clear();
        result
    }

    async fn list_sessions(&self, user_id: i32) -> Result<Vec<api_token::Model>, AppError> {
        // Not cached: the cache holds per-credential lookups, not lists.
        self.inner.list_sessions(user_id).await
    }

    async fn has_session_for_device(
        &self,
        user_id: i32,
        platform: &str,
        device_id: &str,
    ) -> Result<bool, AppError> {
        // Read-only, so it neither consults nor invalidates the credential cache.
        self.inner
            .has_session_for_device(user_id, platform, device_id)
            .await
    }

    async fn find_by_id_and_user(
        &self,
        token_id: i32,
        user_id: i32,
    ) -> Result<Option<api_token::Model>, AppError> {
        self.inner.find_by_id_and_user(token_id, user_id).await
    }

    async fn delete_by_id_and_user(&self, token_id: i32, user_id: i32) -> Result<u64, AppError> {
        let result = self.inner.delete_by_id_and_user(token_id, user_id).await;
        self.cache.clear();
        result
    }

    async fn create_session_token(
        &self,
        params: CreateSessionTokenParams,
    ) -> Result<api_token::Model, AppError> {
        let result = self.inner.create_session_token(params).await;
        self.cache.clear();
        result
    }
}

/// Cache decorator for unified API-key lookups.
///
/// The key row and its library bindings are cached as one value; any mutation clears the cache so
/// a revoked, edited or rebound key takes effect immediately, while `touch_last_used` does not
/// because the auth path never reads `last_used_at` from the cached value. Binding changes via
/// another `Repositories` handle are not observed until the TTL expires, but the auth layer
/// re-checks user state and library membership against the DB on every request.
pub struct CachingApiKeyRepository {
    inner: Arc<dyn ApiKeyRepository>,
    cache: TokenCache<ApiKeyLookup>,
}

impl CachingApiKeyRepository {
    pub fn new(inner: Arc<dyn ApiKeyRepository>) -> Self {
        Self {
            inner,
            cache: TokenCache::new(TOKEN_CACHE_TTL),
        }
    }
}

#[async_trait]
impl ApiKeyRepository for CachingApiKeyRepository {
    async fn find_by_presented(&self, raw: &str) -> Result<Option<ApiKeyLookup>, AppError> {
        if let Some(lookup) = self
            .cache
            .get(raw, |lookup| !token_expired(lookup.key.expires_at))
        {
            return Ok(Some(lookup));
        }
        let result = self.inner.find_by_presented(raw).await?;
        if let Some(lookup) = &result {
            self.cache.insert(raw, lookup.clone());
        }
        Ok(result)
    }

    async fn find_by_id_and_user(
        &self,
        key_id: i32,
        user_id: i32,
    ) -> Result<Option<api_key::Model>, AppError> {
        self.inner.find_by_id_and_user(key_id, user_id).await
    }

    async fn find_by_id(&self, key_id: i32) -> Result<Option<api_key::Model>, AppError> {
        self.inner.find_by_id(key_id).await
    }

    async fn find_by_user(&self, user_id: i32) -> Result<Vec<api_key::Model>, AppError> {
        self.inner.find_by_user(user_id).await
    }

    async fn create(&self, params: CreateApiKeyParams) -> Result<api_key::Model, AppError> {
        let result = self.inner.create(params).await;
        self.cache.clear();
        result
    }

    async fn update_metadata(
        &self,
        key_id: i32,
        user_id: i32,
        params: UpdateApiKeyParams,
    ) -> Result<bool, AppError> {
        let result = self.inner.update_metadata(key_id, user_id, params).await;
        self.cache.clear();
        result
    }

    async fn delete_by_id_and_user(&self, key_id: i32, user_id: i32) -> Result<bool, AppError> {
        let result = self.inner.delete_by_id_and_user(key_id, user_id).await;
        // A revoked key must stop authenticating immediately.
        self.cache.clear();
        result
    }

    async fn delete_by_id(&self, key_id: i32) -> Result<bool, AppError> {
        let result = self.inner.delete_by_id(key_id).await;
        self.cache.clear();
        result
    }

    async fn delete_by_user(&self, user_id: i32) -> Result<u64, AppError> {
        let result = self.inner.delete_by_user(user_id).await;
        self.cache.clear();
        result
    }

    async fn list_bindings(
        &self,
        key_id: i32,
    ) -> Result<Vec<infra::entity::api_key_repo::Model>, AppError> {
        self.inner.list_bindings(key_id).await
    }

    async fn list_bindings_for_keys(
        &self,
        key_ids: &[i32],
    ) -> Result<Vec<infra::entity::api_key_repo::Model>, AppError> {
        self.inner.list_bindings_for_keys(key_ids).await
    }

    async fn replace_bindings(
        &self,
        key_id: i32,
        bindings: &[ApiKeyBinding],
    ) -> Result<(), AppError> {
        let result = self.inner.replace_bindings(key_id, bindings).await;
        self.cache.clear();
        result
    }

    async fn delete_bindings_by_repo(&self, repo_id: &str) -> Result<u64, AppError> {
        let result = self.inner.delete_bindings_by_repo(repo_id).await;
        self.cache.clear();
        result
    }

    async fn delete_bindings_for_repo_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<u64, AppError> {
        let result = self
            .inner
            .delete_bindings_for_repo_user(repo_id, user_id)
            .await;
        self.cache.clear();
        result
    }

    async fn delete_orphan_bound_keys(&self) -> Result<u64, AppError> {
        let result = self.inner.delete_orphan_bound_keys().await;
        self.cache.clear();
        result
    }

    async fn touch_last_used(&self, key_id: i32, ts: i64) -> Result<(), AppError> {
        self.inner.touch_last_used(key_id, ts).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// In-memory `SyncTokenRepository` that counts DB-backed `find_by_token` calls.
    struct MockSyncTokenRepo {
        db_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SyncTokenRepository for MockSyncTokenRepo {
        async fn find_by_token(&self, token: &str) -> Result<Option<sync_token::Model>, AppError> {
            self.db_calls.fetch_add(1, Ordering::SeqCst);
            let model = sync_token::Model {
                id: 1,
                token: token.to_string(),
                repo_id: "repo-1".to_string(),
                user_id: 7,
                created_at: 0,
                expires_at: None,
                peer_id: None,
                peer_name: None,
                peer_ip: None,
                client_version: None,
                last_sync_time: None,
            };
            Ok(Some(model))
        }

        async fn find_by_repo_user_peer(
            &self,
            _repo_id: &str,
            _user_id: i32,
            _peer_id: Option<&str>,
        ) -> Result<Option<sync_token::Model>, AppError> {
            Ok(None)
        }

        async fn find_by_repos_user_peer(
            &self,
            _repo_ids: &[String],
            _user_id: i32,
            _peer_id: Option<&str>,
        ) -> Result<Vec<sync_token::Model>, AppError> {
            Ok(Vec::new())
        }

        async fn find_by_token_and_repo(
            &self,
            _token: &str,
            _repo_id: &str,
        ) -> Result<Option<sync_token::Model>, AppError> {
            Ok(None)
        }

        async fn create(
            &self,
            _repo_id: &str,
            _user_id: i32,
            _token: String,
            _peer: Option<&PeerStamp>,
            _now: i64,
            _expires_at: Option<i64>,
        ) -> Result<(), AppError> {
            Ok(())
        }

        async fn delete_by_repo(&self, _repo_id: &str) -> Result<(), AppError> {
            Ok(())
        }

        async fn delete_by_token(&self, _token: &str) -> Result<(), AppError> {
            Ok(())
        }

        async fn delete_by_id(&self, _id: i32) -> Result<(), AppError> {
            Ok(())
        }

        async fn list_for_user(&self, _user_id: i32) -> Result<Vec<sync_token::Model>, AppError> {
            self.db_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }

        async fn delete_by_id_and_user(&self, _id: i32, _user_id: i32) -> Result<u64, AppError> {
            Ok(0)
        }

        async fn delete_by_user(&self, _user_id: i32) -> Result<u64, AppError> {
            Ok(0)
        }

        async fn delete_by_repo_and_user(
            &self,
            _repo_id: &str,
            _user_id: i32,
        ) -> Result<u64, AppError> {
            Ok(0)
        }

        async fn delete_by_user_and_peer(
            &self,
            _user_id: i32,
            _peer_id: &str,
        ) -> Result<u64, AppError> {
            Ok(0)
        }

        async fn attach_peer_if_unset(
            &self,
            _id: i32,
            _peer: &PeerStamp,
        ) -> Result<bool, AppError> {
            Ok(false)
        }

        async fn touch_peer(
            &self,
            _id: i32,
            _peer_ip: Option<String>,
            _client_version: Option<String>,
            _last_sync_time: i64,
        ) -> Result<(), AppError> {
            Ok(())
        }

        fn reveal_token(&self, model: &sync_token::Model) -> Option<String> {
            Some(model.token.clone())
        }
    }

    #[tokio::test]
    async fn find_by_token_short_circuits_to_cache() {
        let db_calls = Arc::new(AtomicUsize::new(0));
        let repo = CachingSyncTokenRepository::new(Arc::new(MockSyncTokenRepo {
            db_calls: db_calls.clone(),
        }));

        let first = repo.find_by_token("tok-1").await.unwrap().unwrap();
        assert_eq!(first.user_id, 7);
        assert_eq!(db_calls.load(Ordering::SeqCst), 1, "first lookup hits DB");

        let second = repo.find_by_token("tok-1").await.unwrap().unwrap();
        assert_eq!(second.user_id, 7);
        assert_eq!(
            db_calls.load(Ordering::SeqCst),
            1,
            "second lookup must be served from cache"
        );
    }

    #[tokio::test]
    async fn delete_and_create_clear_the_cache() {
        let db_calls = Arc::new(AtomicUsize::new(0));
        let repo = CachingSyncTokenRepository::new(Arc::new(MockSyncTokenRepo {
            db_calls: db_calls.clone(),
        }));

        repo.find_by_token("tok-1").await.unwrap().unwrap();
        assert_eq!(db_calls.load(Ordering::SeqCst), 1);

        repo.delete_by_token("tok-1").await.unwrap();
        repo.find_by_token("tok-1").await.unwrap().unwrap();
        assert_eq!(
            db_calls.load(Ordering::SeqCst),
            2,
            "delete must clear the cache"
        );

        repo.create("repo-1", 7, "tok-2".to_string(), None, 0, None)
            .await
            .unwrap();
        repo.find_by_token("tok-1").await.unwrap().unwrap();
        assert_eq!(
            db_calls.load(Ordering::SeqCst),
            3,
            "create must clear the cache"
        );
    }
}
