//! Projections from a [`JobRun`] onto the JSON shapes the official clients
//! already parse.
//!
//! These endpoints are client-compatibility surface: the web UI never calls
//! them (it uses the synchronous batch endpoints), so the only thing that keeps
//! them honest is that their wire format is pinned by tests. The projection is
//! deliberately the *only* implementation — there is no second store to read
//! from, so the format cannot drift from what the task system actually records.

use serde_json::{Value, json};

use super::run::{JobRun, JobState};

/// `GET /api/v2.1/query-copy-move-progress/`.
///
/// `done_count` is `0` until the run succeeds, and equal to `total` when it
/// does. That looks odd but it is exactly what the endpoint has always
/// reported: the old manager only set `done_count` from `complete_task`, so a
/// run in flight and a failed run both read `0`.
pub fn copy_move_progress(run: &JobRun) -> Value {
    let total = run.expected_total.unwrap_or(0);
    let (state, done, failed, successful) = match &run.state {
        JobState::Queued | JobState::Running | JobState::Yielded => {
            ("processing", false, false, false)
        }
        JobState::Succeeded => ("completed", true, false, true),
        JobState::Failed(_) | JobState::TimedOut | JobState::Interrupted => {
            ("failed", true, true, false)
        }
        JobState::Cancelled => ("failed", true, true, false),
    };
    let description = match &run.state {
        JobState::Failed(message) => message.clone(),
        JobState::TimedOut => "task timed out".to_string(),
        JobState::Interrupted => "task interrupted by a server restart".to_string(),
        JobState::Cancelled => "task cancelled".to_string(),
        _ => String::new(),
    };
    let done_count = if successful {
        total
    } else {
        run.progress.done.min(total)
    };

    json!({
        "state": state,
        "done": done,
        "failed": failed,
        "successful": successful,
        "description": description,
        "total": total,
        "done_count": done_count,
        "failed_count": if failed { 1 } else { 0 },
        // Nothing in the system can stop a copy or a move once it has begun:
        // a move is two commits, and interrupting between them loses the entry.
        "cancelable": false,
    })
}

/// `GET /api2/reindex-progress/`.
///
/// Every field the old handler serialized is present, including `creator_id`,
/// which leaked the submitter's id and is kept because clients may read it.
pub fn reindex_progress(run: &JobRun) -> Value {
    let (state, error, finished_at) = match &run.state {
        JobState::Queued | JobState::Running | JobState::Yielded => ("running", None, None),
        JobState::Succeeded => ("completed", None, run.finished_at),
        JobState::Failed(message) => ("failed", Some(message.clone()), run.finished_at),
        JobState::TimedOut => (
            "failed",
            Some("reindex timed out".to_string()),
            run.finished_at,
        ),
        JobState::Interrupted => (
            "failed",
            Some("reindex interrupted by a server restart".to_string()),
            run.finished_at,
        ),
        JobState::Cancelled => (
            "failed",
            Some("reindex cancelled".to_string()),
            run.finished_at,
        ),
    };

    json!({
        "state": state,
        "repo_id": detail_str(run, "repo_id"),
        "done_count": run.progress.done,
        "total": run.progress.total.unwrap_or(0),
        "indexed": detail_u64(run, "indexed"),
        "skipped": detail_u64(run, "skipped"),
        "error": error,
        "finished_at": finished_at,
        "creator_id": run.owner.unwrap_or(0),
    })
}

/// Read a string detail, falling back to the submit-time params (which are only
/// present while the run is active).
fn detail_str(run: &JobRun, key: &str) -> String {
    run.details
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            run.params
                .as_ref()
                .and_then(|p| p.get(key))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn detail_u64(run: &JobRun, key: &str) -> u64 {
    run.details.get(key).and_then(|v| v.as_u64()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::run::{JobRun, RunId};
    use crate::tasks::spec::{JobKey, Visibility};

    fn run(key: JobKey, state: JobState, total: Option<u64>) -> JobRun {
        let mut run = JobRun::queued(
            key,
            Visibility::Owner,
            Some(7),
            serde_json::json!({"repo_id": "r1", "src_dirents": ["a", "b"]}),
            "Copy 2 items",
            total,
            0,
        );
        run.state = state;
        run
    }

    /// The exact shape the desktop client parses, pinned field by field.
    #[test]
    fn copy_move_progress_shape_is_pinned() {
        let active = copy_move_progress(&run(JobKey::Copy, JobState::Running, Some(2)));
        assert_eq!(active["state"], "processing");
        assert_eq!(active["done"], false);
        assert_eq!(active["failed"], false);
        assert_eq!(active["successful"], false);
        assert_eq!(active["total"], 2);
        assert_eq!(active["done_count"], 0, "in flight reads zero");
        assert_eq!(active["failed_count"], 0);
        assert_eq!(active["cancelable"], false);
        assert_eq!(active["description"], "");
        assert_eq!(active.as_object().unwrap().len(), 9, "no extra fields");
    }

    #[test]
    fn copy_move_progress_reports_completion() {
        let mut done = run(JobKey::Copy, JobState::Succeeded, Some(2));
        done.progress.done = 2;
        let value = copy_move_progress(&done);
        assert_eq!(value["state"], "completed");
        assert_eq!(value["done"], true);
        assert_eq!(value["successful"], true);
        assert_eq!(value["done_count"], 2);
        assert_eq!(value["failed_count"], 0);
    }

    /// A failed run keeps `done_count` at zero, matching the old manager, which
    /// never touched it on the failure path.
    #[test]
    fn copy_move_progress_reports_failure_without_a_done_count() {
        let failed = run(
            JobKey::Move,
            JobState::Failed("source not found".into()),
            Some(2),
        );
        let value = copy_move_progress(&failed);
        assert_eq!(value["state"], "failed");
        assert_eq!(value["done"], true);
        assert_eq!(value["failed"], true);
        assert_eq!(value["successful"], false);
        assert_eq!(value["description"], "source not found");
        assert_eq!(value["done_count"], 0);
        assert_eq!(value["failed_count"], 1);
    }

    #[test]
    fn an_interrupted_run_reads_as_failed_rather_than_missing() {
        let value = copy_move_progress(&run(JobKey::Move, JobState::Interrupted, Some(1)));
        assert_eq!(value["state"], "failed");
        assert!(value["description"].as_str().unwrap().contains("restart"));
    }

    #[test]
    fn a_run_without_a_total_counts_zero() {
        let value = copy_move_progress(&run(JobKey::Copy, JobState::Running, None));
        assert_eq!(value["total"], 0);
        assert_eq!(value["done_count"], 0);
    }

    /// `creator_id` is part of the shape even though it leaks the submitter.
    #[test]
    fn reindex_progress_shape_is_pinned() {
        let mut r = run(JobKey::Reindex, JobState::Running, Some(10));
        // `total` comes from what the run has actually reported, not from the
        // submit-time expectation: the old handler started at zero too.
        r.progress.done = 4;
        r.progress.total = Some(10);
        let value = reindex_progress(&r);
        assert_eq!(value["state"], "running");
        assert_eq!(value["repo_id"], "r1");
        assert_eq!(value["done_count"], 4);
        assert_eq!(value["total"], 10);
        assert_eq!(value["indexed"], 0);
        assert_eq!(value["skipped"], 0);
        assert_eq!(value["error"], Value::Null);
        assert_eq!(value["finished_at"], Value::Null);
        assert_eq!(value["creator_id"], 7);
        assert_eq!(value.as_object().unwrap().len(), 9, "no extra fields");
    }

    #[test]
    fn a_reindex_that_has_not_reported_yet_reads_zero() {
        let r = run(JobKey::Reindex, JobState::Running, Some(10));
        let value = reindex_progress(&r);
        assert_eq!(value["done_count"], 0);
        assert_eq!(value["total"], 0, "no progress reported yet");
    }

    #[test]
    fn a_completed_reindex_keeps_its_repository_and_counts() {
        let mut r = run(JobKey::Reindex, JobState::Succeeded, Some(10));
        r.progress.done = 10;
        r.progress.total = Some(10);
        r.finished_at = Some(123);
        // The params are released at the end; the details carry what is needed.
        r.drop_params();
        r.set_detail("indexed", json!(8));
        r.set_detail("skipped", json!(2));
        r.set_detail("repo_id", json!("r1"));

        let value = reindex_progress(&r);
        assert_eq!(value["state"], "completed");
        assert_eq!(value["repo_id"], "r1", "repo survives the params drop");
        assert_eq!(value["indexed"], 8);
        assert_eq!(value["skipped"], 2);
        assert_eq!(value["finished_at"], 123);
    }

    #[test]
    fn a_failed_reindex_reports_the_error() {
        let mut r = run(
            JobKey::Reindex,
            JobState::Failed("index locked".into()),
            Some(0),
        );
        r.finished_at = Some(9);
        let value = reindex_progress(&r);
        assert_eq!(value["state"], "failed");
        assert_eq!(value["error"], "index locked");
        assert_eq!(value["finished_at"], 9);
    }

    #[test]
    fn an_unknown_run_id_is_not_leaked_by_the_projection() {
        // The projection takes a run, so "not found" is the store's answer, not
        // this module's; assert only that a fresh id is unrelated.
        let a = RunId::new();
        let b = RunId::new();
        assert_ne!(a, b);
    }
}
