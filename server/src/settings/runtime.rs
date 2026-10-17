//! The runtime configuration snapshot.
//!
//! [`RuntimeConfig`] is a handle whose `Arc<Config>` snapshot can be replaced,
//! and every reader goes through [`crate::AppState::config`], so a saved
//! setting reaches all of them at once. Cloning it at the top of a handler (or
//! a service call) costs one atomic increment.
//!
//! Anything captured into a long-lived object at startup — the listener, the
//! database pool, the derived ciphers, the router layers, the rate limiters —
//! is either pushed explicitly through a hook or marked restart-only in the
//! catalog, so only values read from the snapshot follow a replacement.

use std::sync::{Arc, RwLock};

use infra::config::Config;

/// A replaceable configuration snapshot shared by every reader.
#[derive(Clone)]
pub struct RuntimeConfig {
    inner: Arc<Inner>,
}

struct Inner {
    current: RwLock<Arc<Config>>,
}

impl RuntimeConfig {
    pub fn new(config: Config) -> Self {
        Self {
            inner: Arc::new(Inner {
                current: RwLock::new(Arc::new(config)),
            }),
        }
    }

    /// The current snapshot: an `Arc` clone under a read lock, so callers can
    /// take one per request and read several fields from the same consistent view.
    pub fn get(&self) -> Arc<Config> {
        self.inner
            .current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Install a new snapshot. Readers already holding the previous one finish
    /// against it: no request can observe a half-applied configuration.
    pub fn replace(&self, next: Config) {
        let mut current = self
            .inner
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *current = Arc::new(next);
    }

    /// Read a field reader-style: `runtime.config(|c| &c.server.addr)`.
    pub fn with<T>(&self, f: impl FnOnce(&Config) -> T) -> T {
        f(&self.get())
    }
}

impl std::fmt::Debug for RuntimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Config` is redacted, so this cannot leak; spelled out so no future field leaks through it.
        f.debug_struct("RuntimeConfig").finish_non_exhaustive()
    }
}
