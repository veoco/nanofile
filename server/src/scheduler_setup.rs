//! Default background-task registration for the application scheduler.
//!
//! Extracted from `AppState::new` so the state constructor stays readable.
//! All periodic / continuous housekeeping tasks are registered here once,
//! keyed off the config flags they depend on.

use std::sync::Arc;

use crate::fs::core::gc::GcManager;
use crate::indexer::TextIndexer;
use crate::notification::manager::NotificationManager;
use crate::repository::Repositories;
use crate::scheduler::{Scheduler, TaskOutput};
use infra::config::GcConfig;
use infra::crypto::password_manager::PasswordManager;

/// Register the standard startup background tasks on the shared scheduler.
///
/// - Continuous: notification event listener.
/// - Periodic: notification token expiry, password-cache cleanup, expired
///   share/upload-link cleanup, garbage collection, and index commits.
pub fn register_default_tasks(
    scheduler: &Arc<Scheduler>,
    repos: &Arc<Repositories>,
    notification_manager: Option<&NotificationManager>,
    password_manager: &Arc<PasswordManager>,
    gc_config: &GcConfig,
    indexer: Option<&TextIndexer>,
) {
    // Continuous: event listener (forwards repo-update events to WebSocket subscribers).
    if let Some(mgr) = notification_manager {
        let mgr = mgr.clone();
        scheduler.spawn_continuous("event listener", move |token| async move {
            mgr.run_event_listener(token).await;
        });
    }

    // Periodic: JWT token expiry check (hourly).
    if let Some(mgr) = notification_manager {
        let mgr = mgr.clone();
        scheduler.spawn_periodic("token expiry check", 3600, move || {
            let mgr = mgr.clone();
            async move {
                mgr.check_expired_tokens().await;
                TaskOutput::success("ok", None)
            }
        });
    }

    // Periodic: password cache cleanup (every 5 minutes).
    {
        let pm = password_manager.clone();
        scheduler.spawn_periodic("password cache cleanup", 300, move || {
            let pm = pm.clone();
            async move {
                let count = pm.cleanup_expired_once().await;
                if count > 0 {
                    TaskOutput::success(
                        format!("Evicted {count} expired password cache entries"),
                        Some(count),
                    )
                } else {
                    TaskOutput::success("no expired entries", None)
                }
            }
        });
    }

    // Periodic: expired share link cleanup (hourly).
    {
        let repos = repos.clone();
        scheduler.spawn_periodic("share link cleanup", 3600, move || {
            let repos = repos.clone();
            async move {
                let now = chrono::Utc::now().timestamp();
                match repos.share_link.delete_expired(now).await {
                    Ok(count) if count > 0 => TaskOutput::success(
                        format!("Cleaned up {count} expired share links"),
                        Some(count),
                    ),
                    Ok(_) => TaskOutput::success("no expired share links", None),
                    Err(e) => {
                        TaskOutput::error(format!("Failed to clean expired share links: {e}"))
                    }
                }
            }
        });
    }

    // Periodic: expired upload link cleanup (hourly).
    {
        let repos = repos.clone();
        scheduler.spawn_periodic("upload link cleanup", 3600, move || {
            let repos = repos.clone();
            async move {
                let now = chrono::Utc::now().timestamp();
                match repos.upload_link.delete_expired(now).await {
                    Ok(count) if count > 0 => TaskOutput::success(
                        format!("Cleaned up {count} expired upload links"),
                        Some(count),
                    ),
                    Ok(_) => TaskOutput::success("no expired upload links", None),
                    Err(e) => {
                        TaskOutput::error(format!("Failed to clean expired upload links: {e}"))
                    }
                }
            }
        });
    }

    // Periodic: garbage collection (configurable interval).
    if gc_config.enabled {
        let repos = repos.clone();
        scheduler.spawn_periodic("gc", gc_config.interval_hours * 3600, move || {
            let repos = repos.clone();
            async move {
                match GcManager::garbage_collect(&repos).await {
                    Ok(count) if count > 0 => TaskOutput::success(
                        format!("GC removed {count} unreferenced FS objects"),
                        Some(count),
                    ),
                    Ok(_) => TaskOutput::success("GC completed: nothing to remove", None),
                    Err(e) => TaskOutput::error(format!("GC failed: {e}")),
                }
            }
        });
    }

    // Periodic: index background committer (every 30 seconds).
    if let Some(idx) = indexer {
        let idx = idx.clone();
        scheduler.spawn_periodic("index commit", 30, move || {
            let idx = idx.clone();
            async move {
                match idx.commit() {
                    Ok(()) => TaskOutput::success("index committed", None),
                    Err(e) => TaskOutput::error(format!("Background index commit failed: {e}")),
                }
            }
        });
    }
}
