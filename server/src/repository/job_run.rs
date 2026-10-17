//! Durable history for background runs, and the lease claim that makes a run resumable.
//!
//! A row is claimed by setting a lease; the claim is exclusive because a competing worker's
//! update must find the lease free or expired. Backoff, lease and retention numbers are shared
//! with the outbound-mail queue (see [`crate::tasks::queue::QueuePolicy`]).
//!
//! Only a job declared `Durable` stores its input, and the catalog refuses `Durable` for a job
//! that is not idempotent: replaying a destructive run after a crash is worse than losing it.

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};

use base::error::AppError;
use infra::entity::job_run;

/// Which finished runs a reader asked to see.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunHistoryFilter {
    /// A job slug, not a `JobKey`: the journal holds rows for jobs this build no longer runs.
    pub kind: Option<String>,
    pub outcome: Option<RunOutcome>,
}

/// The verdict a reader filters by. Timeouts count as failures: a reader looking for trouble
/// wants everything that did not succeed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Succeeded,
    Failed,
}

/// A row to write when a run is first submitted.
pub struct NewJobRun {
    pub id: String,
    pub kind: String,
    pub owner: Option<i32>,
    pub summary: String,
    pub params: Option<String>,
    pub created_at: i64,
}

/// A run recorded only once it has something to report: a job that keeps only notable runs is
/// never written down before it starts, so its whole row is built at the end. No params, since
/// nothing reads them.
pub struct FinishedJobRun {
    pub id: String,
    pub kind: String,
    pub owner: Option<i32>,
    pub phase: String,
    pub summary: String,
    pub error: Option<String>,
    pub processed: Option<i64>,
    pub attempt: i32,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: i64,
}

#[async_trait]
pub trait JobRunRepository: Send + Sync {
    /// Queue a submitted run under a lease, so a concurrent recovery pass leaves it alone.
    async fn enqueue(&self, run: NewJobRun, lease_until: i64) -> Result<(), AppError>;

    /// Write the terminal state, releasing the lease and dropping the input. A `Some` summary
    /// replaces the submit-time one; `None` keeps what the submitter said.
    async fn finish(
        &self,
        id: &str,
        phase: &str,
        error: Option<&str>,
        processed: Option<i64>,
        summary: Option<&str>,
        finished_at: i64,
    ) -> Result<(), AppError>;

    /// Record a run never written down before it started: its job keeps only the notable runs.
    async fn record_finished(&self, run: FinishedJobRun) -> Result<(), AppError>;

    /// Mark a run executing, so a crash leaves a `running` row rather than a `queued` one.
    async fn mark_running(
        &self,
        id: &str,
        started_at: i64,
        lease_until: i64,
    ) -> Result<(), AppError>;

    /// Unfinished rows whose lease ran out, oldest first: the ones a recovery pass may take over.
    async fn recoverable(&self, now: i64, limit: u64) -> Result<Vec<job_run::Model>, AppError>;

    /// Take a row over for recovery: bump the attempt and renew the lease.
    /// `false` means somebody else got there first.
    async fn claim_for_recovery(
        &self,
        id: &str,
        expected_attempt: i32,
        now: i64,
        lease_until: i64,
    ) -> Result<bool, AppError>;

    /// Newest finished runs, for the administrator's listing.
    async fn recent(&self, limit: u64) -> Result<Vec<job_run::Model>, AppError>;

    /// Newest finished runs a reader asked to see.
    async fn recent_matching(
        &self,
        limit: u64,
        filter: &RunHistoryFilter,
    ) -> Result<Vec<job_run::Model>, AppError>;

    /// The jobs that appear in the newest `window` finished runs, newest first: what a reader can
    /// filter by is what the list holds, and a job with no rows would only show an empty page.
    async fn recent_kinds(&self, window: u64) -> Result<Vec<String>, AppError>;

    /// The newest finished run of each job the `window` reaches, read once at start-up: those
    /// counters are process-lifetime, so the journal is where the last verdict actually lives.
    async fn latest_per_kind(&self, window: u64) -> Result<Vec<job_run::Model>, AppError>;

    /// Retention: drop finished rows past either age cutoff, then the oldest until at most
    /// `max_rows` are left. An unfinished run is never dropped: it is somebody's pending work,
    /// and the store's own bookkeeping reads it.
    async fn prune(
        &self,
        succeeded_before: i64,
        failed_before: i64,
        max_rows: u64,
    ) -> Result<u64, AppError>;
}

pub struct DbJobRunRepository {
    db: Arc<DatabaseConnection>,
}

impl DbJobRunRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

/// Phases that mean the run will not report again on its own.
fn is_terminal(phase: &str) -> bool {
    matches!(
        phase,
        "succeeded" | "failed" | "cancelled" | "timed_out" | "interrupted"
    )
}

#[async_trait]
impl JobRunRepository for DbJobRunRepository {
    async fn enqueue(&self, run: NewJobRun, lease_until: i64) -> Result<(), AppError> {
        job_run::ActiveModel {
            id: Set(run.id),
            kind: Set(run.kind),
            owner: Set(run.owner),
            phase: Set("queued".to_string()),
            summary: Set(run.summary),
            params: Set(run.params),
            error: Set(None),
            processed: Set(None),
            attempt: Set(1),
            created_at: Set(run.created_at),
            started_at: Set(None),
            finished_at: Set(None),
            lease_until: Set(Some(lease_until)),
        }
        .insert(self.db.as_ref())
        .await?;
        Ok(())
    }

    async fn record_finished(&self, run: FinishedJobRun) -> Result<(), AppError> {
        debug_assert!(
            is_terminal(&run.phase),
            "record_finished is for a terminal phase"
        );
        job_run::ActiveModel {
            id: Set(run.id),
            kind: Set(run.kind),
            owner: Set(run.owner),
            phase: Set(run.phase),
            summary: Set(run.summary),
            // Nothing reads a finished run's input, and this one was never replayable work.
            params: Set(None),
            error: Set(run.error),
            processed: Set(run.processed),
            attempt: Set(run.attempt),
            created_at: Set(run.created_at),
            started_at: Set(run.started_at),
            finished_at: Set(Some(run.finished_at)),
            lease_until: Set(None),
        }
        .insert(self.db.as_ref())
        .await?;
        Ok(())
    }

    async fn mark_running(
        &self,
        id: &str,
        started_at: i64,
        lease_until: i64,
    ) -> Result<(), AppError> {
        job_run::Entity::update_many()
            .filter(job_run::Column::Id.eq(id))
            .set(job_run::ActiveModel {
                phase: Set("running".to_string()),
                started_at: Set(Some(started_at)),
                lease_until: Set(Some(lease_until)),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn finish(
        &self,
        id: &str,
        phase: &str,
        error: Option<&str>,
        processed: Option<i64>,
        summary: Option<&str>,
        finished_at: i64,
    ) -> Result<(), AppError> {
        debug_assert!(is_terminal(phase), "finish is for a terminal phase");
        let mut row = job_run::ActiveModel {
            phase: Set(phase.to_string()),
            error: Set(error.map(str::to_string)),
            processed: Set(processed),
            finished_at: Set(Some(finished_at)),
            // The run is over, so nobody holds it and its input is no longer needed.
            lease_until: Set(None),
            params: Set(None),
            ..Default::default()
        };
        // An unset field is left alone, so `None` keeps the submit-time
        // summary rather than blanking the column.
        if let Some(summary) = summary {
            row.summary = Set(summary.to_string());
        }
        job_run::Entity::update_many()
            .filter(job_run::Column::Id.eq(id))
            .set(row)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn recoverable(&self, now: i64, limit: u64) -> Result<Vec<job_run::Model>, AppError> {
        Ok(job_run::Entity::find()
            .filter(job_run::Column::FinishedAt.is_null())
            .filter(
                job_run::Column::LeaseUntil
                    .is_null()
                    .or(job_run::Column::LeaseUntil.lte(now)),
            )
            .order_by_asc(job_run::Column::CreatedAt)
            .limit(limit)
            .all(self.db.as_ref())
            .await?)
    }

    async fn claim_for_recovery(
        &self,
        id: &str,
        expected_attempt: i32,
        now: i64,
        lease_until: i64,
    ) -> Result<bool, AppError> {
        let result = job_run::Entity::update_many()
            .filter(job_run::Column::Id.eq(id))
            .filter(job_run::Column::FinishedAt.is_null())
            .filter(job_run::Column::Attempt.eq(expected_attempt))
            .filter(
                job_run::Column::LeaseUntil
                    .is_null()
                    .or(job_run::Column::LeaseUntil.lte(now)),
            )
            .col_expr(
                job_run::Column::Attempt,
                sea_orm::sea_query::Expr::col(job_run::Column::Attempt).add(1),
            )
            .col_expr(
                job_run::Column::LeaseUntil,
                sea_orm::sea_query::Expr::value(lease_until),
            )
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected == 1)
    }

    async fn recent(&self, limit: u64) -> Result<Vec<job_run::Model>, AppError> {
        self.recent_matching(limit, &RunHistoryFilter::default())
            .await
    }

    async fn recent_matching(
        &self,
        limit: u64,
        filter: &RunHistoryFilter,
    ) -> Result<Vec<job_run::Model>, AppError> {
        let mut query = job_run::Entity::find()
            .filter(job_run::Column::FinishedAt.is_not_null())
            .order_by_desc(job_run::Column::FinishedAt)
            .limit(limit);
        if let Some(kind) = &filter.kind {
            query = query.filter(job_run::Column::Kind.eq(kind.as_str()));
        }
        match filter.outcome {
            Some(RunOutcome::Succeeded) => {
                query = query.filter(job_run::Column::Phase.eq("succeeded"));
            }
            Some(RunOutcome::Failed) => {
                query = query.filter(job_run::Column::Phase.ne("succeeded"));
            }
            None => {}
        }
        Ok(query.all(self.db.as_ref()).await?)
    }

    async fn recent_kinds(&self, window: u64) -> Result<Vec<String>, AppError> {
        Ok(self
            .latest_per_kind(window)
            .await?
            .into_iter()
            .map(|row| row.kind)
            .collect())
    }

    async fn latest_per_kind(&self, window: u64) -> Result<Vec<job_run::Model>, AppError> {
        let mut seen: Vec<String> = Vec::new();
        let mut newest = Vec::new();
        for row in self.recent(window).await? {
            if !seen.contains(&row.kind) {
                seen.push(row.kind.clone());
                newest.push(row);
            }
        }
        Ok(newest)
    }

    async fn prune(
        &self,
        succeeded_before: i64,
        failed_before: i64,
        max_rows: u64,
    ) -> Result<u64, AppError> {
        let by_age = job_run::Entity::delete_many()
            .filter(job_run::Column::FinishedAt.is_not_null())
            .filter(
                job_run::Column::Phase
                    .eq("succeeded")
                    .and(job_run::Column::FinishedAt.lt(succeeded_before))
                    .or(job_run::Column::Phase
                        .ne("succeeded")
                        .and(job_run::Column::FinishedAt.lt(failed_before))),
            )
            .exec(self.db.as_ref())
            .await?
            .rows_affected;

        // Then the count cap. The boundary row is found in the database rather than by reading
        // the table into memory, and everything up to and including it goes, so the cap is exact.
        let by_count = match job_run::Entity::find()
            .filter(job_run::Column::FinishedAt.is_not_null())
            .order_by_desc(job_run::Column::FinishedAt)
            .offset(max_rows)
            .limit(1)
            .one(self.db.as_ref())
            .await?
        {
            Some(row) => match row.finished_at {
                Some(cutoff) => {
                    job_run::Entity::delete_many()
                        .filter(job_run::Column::FinishedAt.is_not_null())
                        .filter(job_run::Column::FinishedAt.lte(cutoff))
                        .exec(self.db.as_ref())
                        .await?
                        .rows_affected
                }
                None => 0,
            },
            None => 0,
        };

        Ok(by_age + by_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use migration::MigratorTrait;

    /// A repository over a real (in-memory) schema, migrated.
    async fn repo() -> DbJobRunRepository {
        let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        migration::Migrator::up(&db, None).await.unwrap();
        DbJobRunRepository::new(Arc::new(db))
    }

    /// A finished row with the fields retention acts on.
    async fn finished(
        repo: &DbJobRunRepository,
        id: &str,
        phase: &str,
        finished_at: i64,
        summary: &str,
    ) {
        repo.record_finished(FinishedJobRun {
            id: id.to_string(),
            kind: "gc".to_string(),
            owner: None,
            phase: phase.to_string(),
            summary: summary.to_string(),
            error: None,
            processed: Some(1),
            attempt: 1,
            created_at: finished_at - 5,
            started_at: Some(finished_at - 5),
            finished_at,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_failure_outlives_a_success_of_the_same_age() {
        let repo = repo().await;
        let now = 1_000_000_000;
        // Old enough to be past the success window, not the failure window.
        let age = 45 * 86_400;
        finished(&repo, "old-success", "succeeded", now - age, "did work").await;
        finished(&repo, "old-failure", "failed", now - age, "did not").await;

        let policy = crate::tasks::queue::JobHistoryPolicy::DEFAULT;
        let (succeeded_before, failed_before) = policy.cutoffs(now);
        assert_eq!(
            repo.prune(succeeded_before, failed_before, 1000)
                .await
                .unwrap(),
            1
        );

        let left = repo.recent(10).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "old-failure");
    }

    /// The count cap drops the oldest rows and leaves the newest alone.
    #[tokio::test]
    async fn the_count_cap_drops_the_oldest_rows() {
        let repo = repo().await;
        let now = 1_000_000_000;
        for i in 0..5 {
            finished(
                &repo,
                &format!("run-{i}"),
                "succeeded",
                now - 10 + i,
                &format!("did {i}"),
            )
            .await;
        }

        // Keep the newest two: the cap lands on the third, and everything up to it goes.
        assert_eq!(repo.prune(0, 0, 2).await.unwrap(), 3);
        let left = repo.recent(10).await.unwrap();
        assert_eq!(left.len(), 2);
        assert_eq!(left[0].id, "run-4");
        assert_eq!(left[0].summary, "did 4", "the run's own report survives");
        assert_eq!(left[1].id, "run-3");

        assert_eq!(repo.prune(0, 0, 2).await.unwrap(), 0);
    }

    /// An unfinished run is somebody's pending work: no cutoff and no count cap may drop it.
    #[tokio::test]
    async fn an_unfinished_run_is_never_pruned() {
        let repo = repo().await;
        repo.enqueue(
            NewJobRun {
                id: "pending".to_string(),
                kind: "reindex".to_string(),
                owner: Some(1),
                summary: "Reindex \"r1\"".to_string(),
                params: Some(serde_json::json!({"repo_id": "r1"}).to_string()),
                created_at: 0,
            },
            -1,
        )
        .await
        .unwrap();

        assert_eq!(repo.prune(i64::MAX, i64::MAX, 0).await.unwrap(), 0);
        assert!(repo.recent(10).await.unwrap().is_empty());
        assert_eq!(repo.recoverable(i64::MAX, 10).await.unwrap().len(), 1);
    }
}
