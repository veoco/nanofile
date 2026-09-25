//! The standard job set, bound to one server generation.
//!
//! The policies come from [`catalog`](super::catalog); this module supplies the
//! bodies. A body captures this generation's resources — the database pool, the
//! block store, the indexer — which is why the whole set is re-installed at
//! every generation rather than registered once for the process.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::fs::core::block_encryption_convert::BlockEncryptionConverter;
use crate::fs::core::gc::{GcManager, GcPolicy};
use crate::handler::web::temp_file::TempFileManager;
use crate::indexer::TextIndexer;
use crate::notification::manager::NotificationManager;
use crate::repository::Repositories;
use crate::service::mail::Mailer;
use crate::tasks::TaskSystem;
use crate::tasks::registry::RegisteredJob;
use crate::tasks::run::{JobFailure, Outcome};
use crate::tasks::spec::{JobKey, JobSpec, ServiceKey, SkipReason, Trigger};
use infra::config::GcConfig;
use infra::crypto::password_manager::PasswordManager;
use infra::storage::DynBlockStorage;
use infra::storage::encrypting_block_store::BlockEncryptionMode;

/// Build and install every job this server generation runs.
///
/// Jobs whose subsystem is switched off are simply not registered: a disabled
/// job has no interval to show and nothing to do. Everything that is registered
/// is registered *unconditionally otherwise*, so flipping a runtime switch does
/// not need a restart.
#[allow(clippy::too_many_arguments)]
pub fn install_default_jobs(
    tasks: &Arc<TaskSystem>,
    shutdown: tokio_util::sync::CancellationToken,
    repos: &Arc<Repositories>,
    db: &Arc<sea_orm::DatabaseConnection>,
    notification_manager: Option<&NotificationManager>,
    password_manager: &Arc<PasswordManager>,
    gc_config: &GcConfig,
    block_store: &DynBlockStorage,
    indexer: Option<&TextIndexer>,
    temp_file_manager: &TempFileManager,
    temp_upload_ttl_hours: u64,
    enc_mode: BlockEncryptionMode,
    block_dir: &Path,
    mail: Option<&Arc<Mailer>>,
) -> Result<(), crate::tasks::registry::RegistryError> {
    // The gauge cannot see the database pool for itself, and the pool belongs
    // to this generation, so the probe is installed with the generation's jobs.
    {
        let db = db.clone();
        tasks.set_db_probe(Arc::new(move || {
            let pool = db.get_sqlite_connection_pool();
            let total = pool.size() as u64;
            let idle = pool.num_idle() as u64;
            let max = pool.options().get_max_connections() as u64;
            (total.saturating_sub(idle), max)
        }));
    }

    // Durable history lives in the database, so a crash is visible rather than
    // silent, and a job declared replayable can be resumed.
    tasks.set_journal(repos.job_run.clone());

    let mut jobs: Vec<RegisteredJob> = Vec::new();
    // The jobs the catalog declares that this generation does not register, with
    // the reason. Recorded on the task system *after* installing, because
    // installing is what clears the record.
    let mut skipped: Vec<(JobKey, SkipReason)> = Vec::new();

    // ── Jobs a request submits ───────────────────────────────────────────
    //
    // These are reference operations: the destination entry points at the
    // source's fs object, so no blocks are copied and the work is one tree
    // update plus a commit. They exist as jobs because the desktop client polls
    // a task id, not because they are long.
    jobs.push(job(JobKey::Copy, {
        let db = db.clone();
        let repos = repos.clone();
        let block_store = block_store.clone();
        let indexer = indexer.cloned();
        move |ctx, params| {
            let db = db.clone();
            let repos = repos.clone();
            let block_store = block_store.clone();
            let indexer = indexer.clone();
            async move {
                let p: BatchParams = parse_params(params)?;
                let user_id = ctx.owner().unwrap_or(0);
                let svc = crate::service::fs::fileops::FileOpsService::new(
                    db,
                    repos,
                    block_store,
                    indexer,
                );
                let copied = svc
                    .batch_copy(
                        &p.repo_id,
                        &p.src_dir,
                        &p.dst_dir,
                        &p.file_names,
                        &p.email,
                        user_id,
                    )
                    .await
                    .map_err(JobFailure::App)?;
                let n = copied.len() as u64;
                Ok(Outcome::success(format!("Copied {n} items"), Some(n)))
            }
        }
    }));

    jobs.push(job(JobKey::Move, {
        let db = db.clone();
        let repos = repos.clone();
        let block_store = block_store.clone();
        let indexer = indexer.cloned();
        move |ctx, params| {
            let db = db.clone();
            let repos = repos.clone();
            let block_store = block_store.clone();
            let indexer = indexer.clone();
            async move {
                let p: BatchParams = parse_params(params)?;
                let user_id = ctx.owner().unwrap_or(0);
                let svc = crate::service::fs::fileops::FileOpsService::new(
                    db,
                    repos,
                    block_store,
                    indexer,
                );
                let moved = svc
                    .batch_move(
                        &p.repo_id,
                        &p.src_dir,
                        &p.dst_dir,
                        &p.file_names,
                        &p.email,
                        user_id,
                    )
                    .await
                    .map_err(JobFailure::App)?;
                let n = moved.len() as u64;
                Ok(Outcome::success(format!("Moved {n} items"), Some(n)))
            }
        }
    }));

    // The full-text rebuild is idempotent — it converges on the same index —
    // which is what lets it be audited and resumed.
    jobs.push(job(JobKey::Reindex, {
        let repos = repos.clone();
        let block_store = block_store.clone();
        let indexer = indexer.cloned();
        move |ctx, params| {
            let repos = repos.clone();
            let block_store = block_store.clone();
            let indexer = indexer.clone();
            async move {
                let repo_id = params
                    .get("repo_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        JobFailure::App(base::error::AppError::BadRequest(
                            "repo_id is required".into(),
                        ))
                    })?
                    .to_string();
                let indexer = indexer.ok_or_else(|| {
                    JobFailure::App(base::error::AppError::BadRequest(
                        "full-text indexing is not enabled".into(),
                    ))
                })?;
                let svc = crate::service::admin::AdminService::new(repos);
                let progress = ctx.clone();
                // The pass checks in before every file, so it parks while the
                // server is busy and resumes when it is not. Those checkpoints
                // are also where a cancellation lands.
                let (indexed, skipped) = svc
                    .reindex(
                        &indexer,
                        &repo_id,
                        &block_store,
                        Some(&ctx),
                        move |done, total| {
                            progress.report(done, Some(total));
                        },
                    )
                    .await
                    .map_err(|e| {
                        // A pass that stopped at a checkpoint comes back as an
                        // opaque failure; the context knows whether it was a
                        // cancellation, which is a different terminal state.
                        if ctx.is_cancelled() {
                            JobFailure::Cancelled
                        } else {
                            JobFailure::App(e)
                        }
                    })?;
                // Kept past the params drop so the progress endpoint can still
                // report which repository this was and what it did.
                ctx.set_detail("indexed", serde_json::json!(indexed));
                ctx.set_detail("skipped", serde_json::json!(skipped));
                Ok(Outcome::success(
                    format!("Indexed {indexed} files, skipped {skipped}"),
                    Some(indexed + skipped),
                ))
            }
        }
    }));

    if let Some(manager) = notification_manager {
        let manager = manager.clone();
        jobs.push(job(JobKey::TokenExpiryCheck, move |_ctx, _params| {
            let manager = manager.clone();
            async move {
                let expired = manager.check_expired_tokens().await;
                Ok(if expired > 0 {
                    Outcome::success(
                        format!("Told {expired} signed-in clients their token expired"),
                        Some(expired),
                    )
                } else {
                    Outcome::success("no token has expired", None)
                })
            }
        }));
    }

    jobs.push(job(JobKey::PasswordCacheCleanup, {
        let manager = password_manager.clone();
        move |_ctx, _params| {
            let manager = manager.clone();
            async move {
                let count = manager.cleanup_expired_once().await;
                Ok(if count > 0 {
                    Outcome::success(
                        format!("Evicted {count} expired password cache entries"),
                        Some(count),
                    )
                } else {
                    Outcome::success("no expired entries", None)
                })
            }
        }
    }));

    jobs.push(job(JobKey::ExpiredTokenCleanup, {
        let db = db.clone();
        move |_ctx, _params| {
            let db = db.clone();
            async move {
                let now = chrono::Utc::now().timestamp();
                match crate::repository::token_cleanup::delete_expired_tokens(db.as_ref(), now)
                    .await
                {
                    Ok(count) if count > 0 => Ok(Outcome::success(
                        format!("Deleted {count} expired tokens"),
                        Some(count),
                    )),
                    Ok(_) => Ok(Outcome::success("no expired tokens", None)),
                    Err(e) => Err(JobFailure::App(e)),
                }
            }
        }
    }));

    jobs.push(job(JobKey::ShareLinkCleanup, {
        let repos = repos.clone();
        move |_ctx, _params| {
            let repos = repos.clone();
            async move {
                let now = chrono::Utc::now().timestamp();
                match repos.share_link.delete_expired(now).await {
                    Ok(count) if count > 0 => Ok(Outcome::success(
                        format!("Cleaned up {count} expired share links"),
                        Some(count),
                    )),
                    Ok(_) => Ok(Outcome::success("no expired share links", None)),
                    Err(e) => Err(JobFailure::App(e)),
                }
            }
        }
    }));

    jobs.push(job(JobKey::UploadLinkCleanup, {
        let repos = repos.clone();
        move |_ctx, _params| {
            let repos = repos.clone();
            async move {
                let now = chrono::Utc::now().timestamp();
                match repos.upload_link.delete_expired(now).await {
                    Ok(count) if count > 0 => Ok(Outcome::success(
                        format!("Cleaned up {count} expired upload links"),
                        Some(count),
                    )),
                    Ok(_) => Ok(Outcome::success("no expired upload links", None)),
                    Err(e) => Err(JobFailure::App(e)),
                }
            }
        }
    }));

    // Garbage collection is registered only when it is switched on: a disabled
    // pass has no interval to show. The switch is a restart-time setting, so the
    // next generation picks up a change.
    if gc_config.enabled {
        let policy = GcPolicy::new(gc_config.min_block_age_secs);
        jobs.push(job_with_trigger(
            JobKey::GarbageCollection,
            Trigger::Periodic {
                interval_secs: gc_config.interval_hours * 3600,
                overlap: super::spec::OverlapPolicy::Skip,
            },
            {
                let repos = repos.clone();
                let block_store = block_store.clone();
                move |ctx, _params| {
                    let repos = repos.clone();
                    let block_store = block_store.clone();
                    async move {
                        // The pass checks in at every repository boundary and
                        // between batches of deletions, so it parks while the
                        // server is busy and resumes when it is not. Those
                        // checkpoints are also where a cancellation lands.
                        let removed = GcManager::garbage_collect_with(
                            &repos,
                            &block_store,
                            policy,
                            Some(&ctx),
                        )
                        .await
                        .map_err(|e| {
                            // An aborted pass comes back as an opaque failure;
                            // the context knows whether it was a cancellation,
                            // which is a different terminal state.
                            if ctx.is_cancelled() {
                                JobFailure::Cancelled
                            } else {
                                JobFailure::App(e)
                            }
                        })?;
                        Ok(if removed > 0 {
                            Outcome::success(
                                format!("GC removed {removed} unreferenced objects/blocks"),
                                Some(removed),
                            )
                        } else {
                            Outcome::success("GC completed: nothing to remove", None)
                        })
                    }
                }
            },
        ));
    }

    // The lazy-migration conversion is registered as a manual job (it also runs
    // once at startup) and only in `Lazy` mode, where pre-existing plaintext
    // blocks can exist. A marker file records completion, so a later run — on
    // the next start or from the admin page — skips with no I/O.
    if enc_mode == BlockEncryptionMode::Lazy {
        const CONVERT_BATCH: usize = 100;
        const CONVERT_BATCH_SLEEP_MS: u64 = 100;
        let block_dir = block_dir.to_path_buf();
        jobs.push(job_with_trigger(
            JobKey::BlockEncryptionConvert,
            Trigger::Manual,
            {
                let block_store = block_store.clone();
                move |ctx, _params| {
                    let block_store = block_store.clone();
                    let block_dir = block_dir.clone();
                    async move {
                        if BlockEncryptionConverter::is_converted(&block_dir).await {
                            return Ok(Outcome::success("already converted, skipped", None));
                        }
                        // The batch size is scaled by the running budget, so a
                        // busy server converts more slowly rather than stopping.
                        let batch = ((CONVERT_BATCH as f32 * ctx.budget()) as usize).max(1);
                        let sleep = Duration::from_millis(
                            (CONVERT_BATCH_SLEEP_MS as f32 / ctx.budget().max(0.05)) as u64,
                        );
                        let converted = ctx
                            .run_blocking(move || {
                                tokio::runtime::Handle::current().block_on(
                                    BlockEncryptionConverter::convert_legacy_blocks(
                                        &block_store,
                                        batch,
                                        sleep,
                                    ),
                                )
                            })
                            .await?
                            .map_err(JobFailure::App)?;
                        let _ = tokio::fs::write(block_dir.join(".encryption_converted"), b"done")
                            .await;
                        Ok(if converted > 0 {
                            Outcome::success(
                                format!("Converted {converted} legacy blocks"),
                                Some(converted),
                            )
                        } else {
                            Outcome::success("no legacy blocks to convert", None)
                        })
                    }
                }
            },
        ));
    }

    // The index committer is a durability deadline: it must run on time, so it
    // is never gated on load and never deferred.
    if let Some(idx) = indexer {
        jobs.push(job(JobKey::IndexCommit, {
            let idx = idx.clone();
            move |_ctx, _params| {
                let idx = idx.clone();
                async move {
                    if !idx.has_pending() {
                        return Ok(Outcome::success("index clean, nothing to commit", None));
                    }
                    tokio::task::spawn_blocking(move || idx.commit())
                        .await
                        .map_err(|e| {
                            JobFailure::App(base::error::AppError::internal(format!(
                                "background index commit task failed: {e}"
                            )))
                        })?
                        .map_err(JobFailure::App)?;
                    Ok(Outcome::success("index committed", None))
                }
            }
        }));
    }

    // The outbox drainer is registered unconditionally and checks the live
    // switch itself, so turning mail on needs no restart.
    if let Some(mail) = mail {
        jobs.push(job(JobKey::MailDelivery, {
            let mail = mail.clone();
            move |_ctx, _params| {
                let mail = mail.clone();
                async move {
                    if !mail.config_enabled() {
                        return Ok(Outcome::success("outbound mail is switched off", None));
                    }
                    match mail.drain_once().await {
                        Ok(report) if report.attempted() == 0 && report.pruned == 0 => {
                            Ok(Outcome::success("no queued mail", None))
                        }
                        Ok(report) => {
                            Ok(Outcome::success(report.summary(), Some(report.delivered)))
                        }
                        Err(e) => Err(JobFailure::App(e)),
                    }
                }
            }
        }));
    }

    jobs.push(job(JobKey::ZipTaskCleanup, |_ctx, _params| async move {
        let dropped =
            crate::handler::web::zip_download::cleanup_expired(chrono::Utc::now().timestamp());
        Ok(if dropped > 0 {
            Outcome::success(
                format!("Dropped {dropped} expired zip downloads"),
                Some(dropped as u64),
            )
        } else {
            Outcome::success("no zip download had expired", None)
        })
    }));

    // Skipped entirely when the TTL is 0, which is what disables the cleanup.
    if temp_upload_ttl_hours > 0 {
        let ttl = Duration::from_secs(temp_upload_ttl_hours * 3600);
        jobs.push(job(JobKey::TempUploadCleanup, {
            let temp_file_manager = temp_file_manager.clone();
            let block_store = block_store.clone();
            let repos = repos.clone();
            move |_ctx, _params| {
                let temp_file_manager = temp_file_manager.clone();
                let block_store = block_store.clone();
                let repos = repos.clone();
                async move {
                    let dropped = temp_file_manager
                        .cleanup_stale(&repos, ttl, &block_store)
                        .await;
                    Ok(if dropped > 0 {
                        Outcome::success(
                            format!("Dropped {dropped} abandoned uploads"),
                            Some(dropped as u64),
                        )
                    } else {
                        Outcome::success("no upload had been abandoned", None)
                    })
                }
            }
        }));
    }

    // The other half of the conditions above: a job whose subsystem is switched
    // off is not registered, which is right and also invisible. Recording why
    // lets the admin listing say "this server does not run it" instead of
    // showing nothing at all.
    if notification_manager.is_none() {
        skipped.push((JobKey::TokenExpiryCheck, SkipReason::NotificationsOff));
    }
    if !gc_config.enabled {
        skipped.push((JobKey::GarbageCollection, SkipReason::GcDisabled));
    }
    if enc_mode != BlockEncryptionMode::Lazy {
        skipped.push((
            JobKey::BlockEncryptionConvert,
            SkipReason::EncryptionNotLazy,
        ));
    }
    if indexer.is_none() {
        skipped.push((JobKey::IndexCommit, SkipReason::IndexOff));
    }
    if mail.is_none() {
        skipped.push((JobKey::MailDelivery, SkipReason::MailOff));
    }
    if temp_upload_ttl_hours == 0 {
        skipped.push((JobKey::TempUploadCleanup, SkipReason::TempUploadTtlZero));
    }

    // Install first: it replaces the generation's job set *and* clears the
    // service listing, so anything registered before it would be forgotten.
    tasks.install(jobs, shutdown)?;

    for (key, reason) in skipped {
        tasks.record_skipped(key, reason);
    }

    // Then resume whatever the previous process left unfinished. Spawned
    // rather than awaited because this runs from a synchronous constructor, and
    // recovery does not have to finish before the server starts serving.
    {
        let tasks = tasks.clone();
        tokio::spawn(async move { tasks.recover().await });
    }

    // The event listener is a service, not a job: it has no owner, no progress
    // and no terminal state, so it gets a lifecycle rather than a run record.
    if let Some(manager) = notification_manager {
        let manager = manager.clone();
        tasks.spawn_service(ServiceKey::EventListener, move |token| async move {
            manager.run_event_listener(token).await;
        });
    }

    Ok(())
}

/// The request payload the copy/move handlers submit.
#[derive(serde::Deserialize)]
struct BatchParams {
    repo_id: String,
    src_dir: String,
    dst_dir: String,
    file_names: Vec<String>,
    email: String,
}

fn parse_params(params: crate::tasks::run::Params) -> Result<BatchParams, JobFailure> {
    serde_json::from_value(params).map_err(|e| {
        JobFailure::App(base::error::AppError::BadRequest(format!(
            "invalid job params: {e}"
        )))
    })
}

/// Pair a catalog policy with a body, keeping the catalog's trigger.
fn job<F, Fut>(key: JobKey, run: F) -> RegisteredJob
where
    F: Fn(crate::tasks::context::JobContext, crate::tasks::run::Params) -> Fut
        + Send
        + Sync
        + 'static,
    Fut: std::future::Future<Output = Result<Outcome, JobFailure>> + Send + 'static,
{
    RegisteredJob::new(super::catalog::policy(key), into_run_fn(run))
}

/// Pair a catalog policy with a body, replacing the trigger.
fn job_with_trigger<F, Fut>(key: JobKey, trigger: Trigger, run: F) -> RegisteredJob
where
    F: Fn(crate::tasks::context::JobContext, crate::tasks::run::Params) -> Fut
        + Send
        + Sync
        + 'static,
    Fut: std::future::Future<Output = Result<Outcome, JobFailure>> + Send + 'static,
{
    let mut spec: JobSpec = super::catalog::policy(key);
    spec.trigger = trigger;
    RegisteredJob::new(spec, into_run_fn(run))
}

/// Erase a body's concrete future type into the boxed [`JobRunFn`].
///
/// [`JobRunFn`]: super::spec::JobRunFn
fn into_run_fn<F, Fut>(run: F) -> super::spec::JobRunFn
where
    F: Fn(crate::tasks::context::JobContext, crate::tasks::run::Params) -> Fut
        + Send
        + Sync
        + 'static,
    Fut: std::future::Future<Output = Result<Outcome, JobFailure>> + Send + 'static,
{
    Arc::new(move |ctx, params| Box::pin(run(ctx, params)))
}
