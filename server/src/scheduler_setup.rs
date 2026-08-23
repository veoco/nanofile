//! Default background-task registration for the application scheduler.
//!
//! Extracted from `AppState::new` so the state constructor stays readable.
//! All periodic / continuous housekeeping tasks are registered here once,
//! keyed off the config flags they depend on.

use std::sync::Arc;

use crate::fs::core::gc::GcManager;
use crate::handler::web::temp_file::TempFileManager;
use crate::indexer::TextIndexer;
use crate::notification::manager::NotificationManager;
use crate::repository::Repositories;
use crate::scheduler::{Scheduler, TaskOutput};
use infra::config::GcConfig;
use infra::crypto::password_manager::PasswordManager;
use infra::storage::DynBlockStorage;

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
    block_store: &DynBlockStorage,
    indexer: Option<&TextIndexer>,
    temp_file_manager: &TempFileManager,
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

    // Periodic: garbage collection (configurable interval). Runs on a blocking
    // thread so the JSON parsing / DB / block I/O never stalls the runtime.
    if gc_config.enabled {
        let repos = repos.clone();
        let block_store = block_store.clone();
        scheduler.spawn_periodic("gc", gc_config.interval_hours * 3600, move || {
            let repos = repos.clone();
            let block_store = block_store.clone();
            async move {
                let result = tokio::task::spawn_blocking(move || {
                    tokio::runtime::Handle::current()
                        .block_on(GcManager::garbage_collect(&repos, &block_store))
                })
                .await;
                match result {
                    Ok(Ok(count)) if count > 0 => TaskOutput::success(
                        format!("GC removed {count} unreferenced objects/blocks"),
                        Some(count),
                    ),
                    Ok(Ok(_)) => TaskOutput::success("GC completed: nothing to remove", None),
                    Ok(Err(e)) => TaskOutput::error(format!("GC failed: {e}")),
                    Err(e) => TaskOutput::error(format!("GC task join failed: {e}")),
                }
            }
        });
    }

    // Periodic: index background committer (every 30 seconds).
    // Runs on a blocking thread so the Tantivy fsync doesn't stall the runtime.
    if let Some(idx) = indexer {
        let idx = idx.clone();
        scheduler.spawn_periodic("index commit", 30, move || {
            let idx = idx.clone();
            async move {
                match tokio::task::spawn_blocking(move || idx.commit()).await {
                    Ok(Ok(())) => TaskOutput::success("index committed", None),
                    Ok(Err(e)) => TaskOutput::error(format!("Background index commit failed: {e}")),
                    Err(e) => {
                        TaskOutput::error(format!("Background index commit task failed: {e}"))
                    }
                }
            }
        });
    }

    // Periodic: purge abandoned zip-download tasks (every 10 minutes).
    scheduler.spawn_periodic("zip task cleanup", 600, || async {
        crate::handler::web::zip_download::cleanup_expired(chrono::Utc::now().timestamp());
        TaskOutput::success("ok", None)
    });

    // Periodic: clean stale resumable-upload temp files (every 30 minutes).
    {
        let tmp = temp_file_manager.clone();
        scheduler.spawn_periodic("temp upload cleanup", 1800, move || {
            let tmp = tmp.clone();
            async move {
                tmp.cleanup_stale(std::time::Duration::from_secs(24 * 3600))
                    .await;
                TaskOutput::success("ok", None)
            }
        });
    }
}
