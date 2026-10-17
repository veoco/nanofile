//! In-memory TTL cache for the left-panel repo list.
//!
//! `build_page_ctx` (~15 web UI pages) and the file-preview handlers re-query
//! the user's left-panel repos on every render — two DB queries each — so this
//! serves the same user's `LeftPanelRepo` list for 60 seconds.
//!
//! The 60s TTL bounds staleness of `size_display`, the one field that changes on
//! the hot upload/sync paths, which deliberately do **not** invalidate. Repo and
//! membership writers (`handler/repos.rs`, `handler/dir.rs`) call `clear_all()`
//! on success, so a change is visible immediately; keep that list in sync. At
//! `MAX_CACHED_USERS` entries the whole cache is dropped, bounding memory.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base::error::AppError;

use crate::repository::Repositories;
use crate::service::repo::service::{LeftPanelRepo, load_left_panel_repos};

/// How long a cached left-panel list is considered fresh.
const LEFT_PANEL_TTL: Duration = Duration::from_secs(60);
/// Upper bound on cached users; at this many entries the whole cache clears.
const MAX_CACHED_USERS: usize = 1024;

struct CachedEntry {
    repos: Vec<LeftPanelRepo>,
    cached_at: Instant,
}

pub struct LeftPanelRepoCache {
    entries: Mutex<HashMap<i32, CachedEntry>>,
    ttl: Duration,
    max_entries: usize,
}

impl Default for LeftPanelRepoCache {
    fn default() -> Self {
        Self::new(LEFT_PANEL_TTL, MAX_CACHED_USERS)
    }
}

impl LeftPanelRepoCache {
    /// `ttl`/`max_entries` are injectable so tests can control expiry and capacity.
    fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            max_entries,
        }
    }

    /// Return the cached list for `user_id`, or load it via `load` on a miss.
    ///
    /// The load runs outside the lock so rendering isn't serialized; concurrent
    /// misses for the same user each load once and the last writer wins.
    pub async fn get<F, Fut>(&self, user_id: i32, load: F) -> Result<Vec<LeftPanelRepo>, AppError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<LeftPanelRepo>, AppError>>,
    {
        if let Some(cached) = self.try_get(user_id) {
            return Ok(cached);
        }
        let loaded = load().await?;
        self.insert(user_id, loaded.clone());
        Ok(loaded)
    }

    /// Production convenience wrapper loading through `load_left_panel_repos`.
    pub async fn get_for_user(
        &self,
        repos: &Repositories,
        user_id: i32,
    ) -> Result<Vec<LeftPanelRepo>, AppError> {
        self.get(user_id, || load_left_panel_repos(repos, user_id))
            .await
    }

    /// Pure TTL hit test (no DB). Evicts the entry when it has expired.
    fn try_get(&self, user_id: i32) -> Option<Vec<LeftPanelRepo>> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        match entries.get(&user_id) {
            Some(e) if e.cached_at.elapsed() <= self.ttl => Some(e.repos.clone()),
            Some(_) => {
                entries.remove(&user_id);
                None
            }
            None => None,
        }
    }

    fn insert(&self, user_id: i32, repos: Vec<LeftPanelRepo>) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= self.max_entries {
            entries.clear();
        }
        entries.insert(
            user_id,
            CachedEntry {
                repos,
                cached_at: Instant::now(),
            },
        );
    }

    /// Drop every cached entry, called by the repo/membership write handlers.
    pub fn clear_all(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn left_panel_repo(id: &str) -> LeftPanelRepo {
        LeftPanelRepo {
            id: id.to_string(),
            name: id.to_string(),
            size_display: "1 B".to_string(),
        }
    }

    #[tokio::test]
    async fn hit_does_not_reload() {
        let cache = LeftPanelRepoCache::new(Duration::from_secs(60), 10);
        let calls = Arc::new(AtomicUsize::new(0));

        for _ in 0..2 {
            let n = calls.clone();
            let result = cache
                .get(1, || {
                    let n = n.clone();
                    async move {
                        n.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![left_panel_repo("r1")])
                    }
                })
                .await
                .unwrap();
            assert_eq!(result.len(), 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "second get must be a hit");
    }

    #[tokio::test]
    async fn clear_all_forces_reload() {
        let cache = LeftPanelRepoCache::new(Duration::from_secs(60), 10);
        let calls = Arc::new(AtomicUsize::new(0));

        cache
            .get(1, {
                let n = calls.clone();
                || async move {
                    n.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![left_panel_repo("r1")])
                }
            })
            .await
            .unwrap();
        cache.clear_all();
        cache
            .get(1, {
                let n = calls.clone();
                || async move {
                    n.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![left_panel_repo("r1")])
                }
            })
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "clear_all must force a reload"
        );
    }

    #[tokio::test]
    async fn expired_entry_reloads() {
        let cache = LeftPanelRepoCache::new(Duration::ZERO, 10);
        let calls = Arc::new(AtomicUsize::new(0));

        cache
            .get(1, {
                let n = calls.clone();
                || async move {
                    n.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![left_panel_repo("r1")])
                }
            })
            .await
            .unwrap();
        cache
            .get(1, {
                let n = calls.clone();
                || async move {
                    n.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![left_panel_repo("r1")])
                }
            })
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "expired entry must reload");
    }

    #[tokio::test]
    async fn different_users_isolated() {
        let cache = LeftPanelRepoCache::new(Duration::from_secs(60), 10);
        let calls = Arc::new(AtomicUsize::new(0));

        for uid in [1, 2] {
            cache
                .get(uid, {
                    let n = calls.clone();
                    || async move {
                        n.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![left_panel_repo("r1")])
                    }
                })
                .await
                .unwrap();
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "each user must load independently"
        );
    }

    #[tokio::test]
    async fn capacity_clears_all() {
        let cache = LeftPanelRepoCache::new(Duration::from_secs(60), 2);
        cache.insert(1, vec![left_panel_repo("r1")]);
        cache.insert(2, vec![left_panel_repo("r2")]);
        cache.insert(3, vec![left_panel_repo("r3")]);

        assert!(cache.try_get(1).is_none(), "u1 evicted by capacity clear");
        assert!(cache.try_get(2).is_none(), "u2 evicted by capacity clear");
        assert!(cache.try_get(3).is_some(), "u3 should be present");
    }
}
