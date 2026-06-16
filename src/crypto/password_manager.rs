use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::crypto::key_derivation;
use crate::crypto::verify::verify_repo_password;
use crate::error::AppError;

/// A cached decryption key entry for an encrypted repo.
#[derive(Debug, Clone)]
struct CachedDecryptKey {
    /// The actual encryption key (32 bytes for AES-256).
    enc_key: Vec<u8>,
    /// The IV for block encryption/decryption (16 bytes).
    enc_iv: Vec<u8>,
    /// UNIX timestamp when this entry expires.
    expires_at: i64,
}

/// In-memory password manager for encrypted repositories.
///
/// This caches decryption keys per user+repo, matching the behavior of
/// seafile-server's `passwd-mgr.c`. Keys are cached for a configurable
/// TTL (default 3600 seconds = 1 hour) and the cache is bounded by
/// `max_entries` (default 10_000) to prevent unbounded growth.
///
/// Key format: `"repo_id:user_id"` composite key.
pub struct PasswordManager {
    cache: Arc<RwLock<HashMap<String, CachedDecryptKey>>>,
    ttl_secs: i64,
    max_entries: usize,
}

impl PasswordManager {
    /// Create a new PasswordManager with the default 1-hour TTL and 10k max entries.
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl_secs: 3600,
            max_entries: 10_000,
        }
    }

    /// Create a new PasswordManager with a custom TTL and 10k max entries.
    pub fn new_with_ttl(ttl_secs: i64) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl_secs,
            max_entries: 10_000,
        }
    }

    /// Create a new PasswordManager with a custom TTL and capacity.
    pub fn new_with_capacity(ttl_secs: i64, max_entries: usize) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::with_capacity(max_entries))),
            ttl_secs,
            max_entries,
        }
    }

    /// Build the composite cache key.
    fn cache_key(repo_id: &str, user_id: i32) -> String {
        format!("{}:{}", repo_id, user_id)
    }

    /// Set a repo password for a user.
    ///
    /// This will:
    /// 1. Verify the password against the stored `magic`
    /// 2. Derive the decryption key from `magic` and `random_key`
    /// 3. Cache the decryption key with expiry
    ///
    /// Returns `Ok(())` on success, or `AppError::RepoPasswdRequired` on
    /// password mismatch, or `AppError::BadRequest` for unsupported versions.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_password(
        &self,
        repo_id: &str,
        user_id: i32,
        password: &str,
        enc_version: i32,
        magic: &str,
        random_key: &str,
        salt: &str,
    ) -> Result<(), AppError> {
        // For enc_version 2, salt is empty (uses default MAGIC_SALT)
        let effective_salt = match enc_version {
            2 => "",
            4 => salt,
            _ => {
                return Err(AppError::BadRequest(format!(
                    "unsupported encryption version: {enc_version}"
                )));
            }
        };

        let magic_owned = magic.to_string();
        if !verify_repo_password(repo_id, password, enc_version, effective_salt, &magic_owned) {
            return Err(AppError::RepoPasswdRequired);
        }

        // 2. Decrypt the random_key to get the actual file encryption key
        let (enc_key, enc_iv) =
            key_derivation::decrypt_repo_enc_key(password, random_key, enc_version, effective_salt)
                .map_err(|e| AppError::BadRequest(format!("key derivation failed: {e}")))?;

        // 3. Cache the decrypted key with expiry
        let now = chrono::Utc::now().timestamp();
        let cached = CachedDecryptKey {
            enc_key,
            enc_iv,
            expires_at: now + self.ttl_secs,
        };

        let mut cache = self.cache.write().await;
        cache.insert(Self::cache_key(repo_id, user_id), cached);

        // Enforce capacity bound: if cache exceeds max_entries, evict expired
        // entries first, then remove the oldest entries.
        if cache.len() > self.max_entries {
            let now = chrono::Utc::now().timestamp();
            cache.retain(|_, entry| now < entry.expires_at);
            // If still over capacity, drain oldest entries (HashMap iteration
            // order is non-deterministic but this is best-effort eviction).
            while cache.len() > self.max_entries {
                let oldest_key = cache
                    .iter()
                    .min_by_key(|(_, v)| v.expires_at)
                    .map(|(k, _)| k.clone());
                if let Some(k) = oldest_key {
                    cache.remove(&k);
                } else {
                    break;
                }
            }
        }

        Ok(())
    }

    /// Check if a password has been set for this user+repo pair.
    ///
    /// Returns `true` if there is a valid (non-expired) cached key.
    pub async fn is_password_set(&self, repo_id: &str, user_id: i32) -> bool {
        let cache = self.cache.read().await;
        let key = Self::cache_key(repo_id, user_id);
        match cache.get(&key) {
            Some(entry) => {
                let now = chrono::Utc::now().timestamp();
                now < entry.expires_at
            }
            None => false,
        }
    }

    /// Get the cached decryption key for web file access.
    ///
    /// Returns `Some((enc_key, enc_iv))` if a valid cached key exists,
    /// or `None` if no password has been set or the cached entry has expired.
    pub async fn get_decrypt_key(&self, repo_id: &str, user_id: i32) -> Option<(Vec<u8>, Vec<u8>)> {
        let cache = self.cache.read().await;
        let key = Self::cache_key(repo_id, user_id);
        match cache.get(&key) {
            Some(entry) => {
                let now = chrono::Utc::now().timestamp();
                if now < entry.expires_at {
                    Some((entry.enc_key.clone(), entry.enc_iv.clone()))
                } else {
                    None
                }
            }
            None => None,
        }
    }

    /// Remove a cached password entry.
    pub async fn remove_password(&self, repo_id: &str, user_id: i32) {
        let mut cache = self.cache.write().await;
        cache.remove(&Self::cache_key(repo_id, user_id));
    }

    /// Remove all cached passwords for a repo (e.g. when repo is deleted).
    pub async fn remove_repo(&self, repo_id: &str) {
        let mut cache = self.cache.write().await;
        let prefix = format!("{}:", repo_id);
        cache.retain(|k, _| !k.starts_with(&prefix));
    }

    /// Remove all cached passwords for a user (e.g. on logout).
    pub async fn remove_user(&self, user_id: i32) {
        let mut cache = self.cache.write().await;
        let suffix = format!(":{}", user_id);
        cache.retain(|k, _| !k.ends_with(&suffix));
    }

    /// Run periodic cleanup of expired entries.
    ///
    /// This is intended to be called from a background tokio task.
    /// The interval controls how often expired entries are purged.
    /// Pass a `CancellationToken` to allow graceful shutdown.
    pub async fn cleanup_expired(&self, token: tokio_util::sync::CancellationToken) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let now = chrono::Utc::now().timestamp();
                    let mut cache = self.cache.write().await;
                    cache.retain(|_, entry| now < entry.expires_at);
                }
                _ = token.cancelled() => {
                    break;
                }
            }
        }
    }
}

impl Default for PasswordManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_magic_and_key() -> (String, String, String) {
        let repo_id = "test-repo-id-123456789012345678901234567890123456";
        let password = "test-password";
        let repo_salt = ""; // use default MAGIC_SALT for v2

        let magic = key_derivation::generate_magic(repo_id, password, 2, repo_salt).unwrap();
        let random_key =
            key_derivation::generate_random_key_for_repo(password, 2, repo_salt).unwrap();
        (magic, random_key, repo_salt.to_string())
    }

    #[tokio::test]
    async fn test_set_and_check_password() {
        let mgr = PasswordManager::new();
        let (magic, random_key, salt) = create_test_magic_and_key();

        let result = mgr
            .set_password(
                "test-repo-id-123456789012345678901234567890123456",
                1,
                "test-password",
                2,
                &magic,
                &random_key,
                &salt,
            )
            .await;
        assert!(result.is_ok());

        assert!(
            mgr.is_password_set("test-repo-id-123456789012345678901234567890123456", 1,)
                .await
        );
    }

    #[tokio::test]
    async fn test_set_password_wrong() {
        let mgr = PasswordManager::new();
        let (magic, random_key, salt) = create_test_magic_and_key();

        let result = mgr
            .set_password(
                "test-repo-id-123456789012345678901234567890123456",
                1,
                "wrong-password",
                2,
                &magic,
                &random_key,
                &salt,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_decrypt_key() {
        let mgr = PasswordManager::new();
        let repo_id = "test-repo-id-123456789012345678901234567890123456";
        let (magic, random_key, salt) = create_test_magic_and_key();

        mgr.set_password(repo_id, 1, "test-password", 2, &magic, &random_key, &salt)
            .await
            .unwrap();

        let key = mgr.get_decrypt_key(repo_id, 1).await;
        assert!(key.is_some());
        let (enc_key, enc_iv) = key.unwrap();
        assert_eq!(enc_key.len(), 32);
        assert_eq!(enc_iv.len(), 16);
    }

    #[tokio::test]
    async fn test_get_decrypt_key_not_set() {
        let mgr = PasswordManager::new();
        let key = mgr.get_decrypt_key("some-repo", 1).await;
        assert!(key.is_none());
    }

    #[tokio::test]
    async fn test_remove_password() {
        let mgr = PasswordManager::new();
        let repo_id = "test-repo-id-123456789012345678901234567890123456";
        let (magic, random_key, salt) = create_test_magic_and_key();

        mgr.set_password(repo_id, 1, "test-password", 2, &magic, &random_key, &salt)
            .await
            .unwrap();
        assert!(mgr.is_password_set(repo_id, 1).await);

        mgr.remove_password(repo_id, 1).await;
        assert!(!mgr.is_password_set(repo_id, 1).await);
    }

    #[tokio::test]
    async fn test_different_users_independent() {
        let mgr = PasswordManager::new();
        let repo_id = "test-repo-id-123456789012345678901234567890123456";
        let (magic, random_key, salt) = create_test_magic_and_key();

        mgr.set_password(repo_id, 1, "test-password", 2, &magic, &random_key, &salt)
            .await
            .unwrap();
        mgr.set_password(repo_id, 2, "test-password", 2, &magic, &random_key, &salt)
            .await
            .unwrap();

        assert!(mgr.is_password_set(repo_id, 1).await);
        assert!(mgr.is_password_set(repo_id, 2).await);

        mgr.remove_user(1).await;
        assert!(!mgr.is_password_set(repo_id, 1).await);
        assert!(mgr.is_password_set(repo_id, 2).await);
    }

    #[tokio::test]
    async fn test_new_with_ttl() {
        let mgr = PasswordManager::new_with_ttl(3600);
        let repo_id = "test-repo-id-123456789012345678901234567890123456";
        let (magic, random_key, salt) = create_test_magic_and_key();
        mgr.set_password(repo_id, 1, "test-password", 2, &magic, &random_key, &salt)
            .await
            .unwrap();
        assert!(mgr.is_password_set(repo_id, 1).await);
    }

    #[tokio::test]
    async fn test_remove_repo() {
        let mgr = PasswordManager::new();
        let repo_id = "test-repo-id-123456789012345678901234567890123456";
        let (magic, random_key, salt) = create_test_magic_and_key();

        mgr.set_password(repo_id, 1, "test-password", 2, &magic, &random_key, &salt)
            .await
            .unwrap();
        mgr.set_password(repo_id, 2, "test-password", 2, &magic, &random_key, &salt)
            .await
            .unwrap();

        mgr.remove_repo(repo_id).await;
        assert!(!mgr.is_password_set(repo_id, 1).await);
        assert!(!mgr.is_password_set(repo_id, 2).await);
    }

    #[tokio::test]
    async fn test_password_not_set_after_expiry() {
        let mgr = PasswordManager::new_with_ttl(0);
        let repo_id = "test-repo-id-123456789012345678901234567890123456";
        let (magic, random_key, salt) = create_test_magic_and_key();
        mgr.set_password(repo_id, 1, "test-password", 2, &magic, &random_key, &salt)
            .await
            .unwrap();

        // TTL of 0 means it expired immediately
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!mgr.is_password_set(repo_id, 1).await);
    }

    #[tokio::test]
    async fn test_set_password_v4() {
        let mgr = PasswordManager::new();
        let repo_id = "test-repo-id-123456789012345678901234567890123456";
        let repo_salt = key_derivation::generate_repo_salt();
        let password = "test-password";

        let magic = key_derivation::generate_magic(repo_id, password, 4, &repo_salt).unwrap();
        let random_key =
            key_derivation::generate_random_key_for_repo(password, 4, &repo_salt).unwrap();

        mgr.set_password(repo_id, 1, password, 4, &magic, &random_key, &repo_salt)
            .await
            .unwrap();
        assert!(mgr.is_password_set(repo_id, 1).await);

        let key = mgr.get_decrypt_key(repo_id, 1).await;
        assert!(key.is_some());
        let (enc_key, enc_iv) = key.unwrap();
        assert_eq!(enc_key.len(), 32);
        assert_eq!(enc_iv.len(), 16);
    }
}
