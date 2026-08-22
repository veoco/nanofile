//! In-memory TTL cache decorators for token lookups.
//!
//! The sync protocol issues many small authenticated requests (check-blocks,
//! get-block, fs-id-list, …). Every one of those requests looked up the token
//! in the DB, adding a per-request round-trip + SQLite lock wait on the hottest
//! path. These decorators cache `find_by_token` results keyed by the raw token
//! string so the hot path short-circuits to memory.
//!
//! # Correctness / security
//!
//! - TTL (5 min) bounds staleness.
//! - A cached entry whose `expires_at` has passed is evicted on read.
//! - Any `create`/`delete*` clears the whole cache (rare operations), so a
//!   regenerated or revoked token takes effect immediately.
//! - `update_peer_info` does **not** clear the cache: it is called on nearly
//!   every sync request and only touches peer fields that the auth layer never
//!   reads from the cached model.
//! - The auth layer still checks user existence + `is_active` against the DB on
//!   every request, so a stale cached token cannot authenticate a deleted or
//!   deactivated account (defense in depth).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use base::error::AppError;
use infra::entity::{api_token, sync_token};

use super::api_token::{ApiTokenRepository, CreateSessionTokenParams};
use super::sync_token::SyncTokenRepository;

/// How long a cached `find_by_token` result is considered fresh.
const TOKEN_CACHE_TTL: Duration = Duration::from_secs(300);

struct CachedEntry<T> {
    value: T,
    cached_at: Instant,
}

/// A small TTL key-value cache with eviction on read for stale entries.
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

    /// Return a fresh cached value for `key`, or `None` if absent/expired.
    /// `is_valid` lets the caller add value-specific validity (e.g. token
    /// expiry); a value failing it is evicted.
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
    async fn find_by_repo_and_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Option<sync_token::Model>, AppError> {
        self.inner.find_by_repo_and_user(repo_id, user_id).await
    }

    async fn find_by_repos_and_user(
        &self,
        repo_ids: &[String],
        user_id: i32,
    ) -> Result<Vec<sync_token::Model>, AppError> {
        self.inner.find_by_repos_and_user(repo_ids, user_id).await
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
        client_peername: Option<String>,
        now: i64,
        expires_at: Option<i64>,
    ) -> Result<(), AppError> {
        let result = self
            .inner
            .create(repo_id, user_id, token, client_peername, now, expires_at)
            .await;
        // A regenerated token may shadow a cached one for the same device.
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

    async fn delete_by_user(&self, user_id: i32) -> Result<u64, AppError> {
        let result = self.inner.delete_by_user(user_id).await;
        self.cache.clear();
        result
    }

    async fn delete_by_user_and_peer(&self, user_id: i32, peer_id: &str) -> Result<u64, AppError> {
        let result = self.inner.delete_by_user_and_peer(user_id, peer_id).await;
        self.cache.clear();
        result
    }

    async fn update_peer_info(
        &self,
        model: sync_token::Model,
        peer_id: Option<String>,
        peer_name: Option<String>,
        peer_ip: Option<String>,
        client_version: Option<String>,
        last_sync_time: Option<i64>,
    ) -> Result<(), AppError> {
        self.inner
            .update_peer_info(
                model,
                peer_id,
                peer_name,
                peer_ip,
                client_version,
                last_sync_time,
            )
            .await
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

    async fn find_by_user_id_with_platform(
        &self,
        user_id: i32,
    ) -> Result<Vec<api_token::Model>, AppError> {
        self.inner.find_by_user_id_with_platform(user_id).await
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

    async fn create_session_token(
        &self,
        params: CreateSessionTokenParams,
    ) -> Result<api_token::Model, AppError> {
        let result = self.inner.create_session_token(params).await;
        self.cache.clear();
        result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Minimal in-memory `SyncTokenRepository` recording how many times the
    /// DB-backed `find_by_token` was called.
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

        async fn find_by_repo_and_user(
            &self,
            _repo_id: &str,
            _user_id: i32,
        ) -> Result<Option<sync_token::Model>, AppError> {
            Ok(None)
        }

        async fn find_by_repos_and_user(
            &self,
            _repo_ids: &[String],
            _user_id: i32,
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
            _client_peername: Option<String>,
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

        async fn delete_by_user(&self, _user_id: i32) -> Result<u64, AppError> {
            Ok(0)
        }

        async fn delete_by_user_and_peer(
            &self,
            _user_id: i32,
            _peer_id: &str,
        ) -> Result<u64, AppError> {
            Ok(0)
        }

        async fn update_peer_info(
            &self,
            _model: sync_token::Model,
            _peer_id: Option<String>,
            _peer_name: Option<String>,
            _peer_ip: Option<String>,
            _client_version: Option<String>,
            _last_sync_time: Option<i64>,
        ) -> Result<(), AppError> {
            Ok(())
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

        // Warm the cache.
        repo.find_by_token("tok-1").await.unwrap().unwrap();
        assert_eq!(db_calls.load(Ordering::SeqCst), 1);

        // Deleting the token invalidates the cached entry → next lookup goes to DB.
        repo.delete_by_token("tok-1").await.unwrap();
        repo.find_by_token("tok-1").await.unwrap().unwrap();
        assert_eq!(
            db_calls.load(Ordering::SeqCst),
            2,
            "delete must clear the cache"
        );

        // Same for create (a regenerated token shadows any stale cached one).
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
