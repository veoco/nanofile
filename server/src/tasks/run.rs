//! The record of one execution of a job: identity, state, progress, ownership.
//!
//! Created by [`super::TaskSystem::submit`] and driven to a terminal state by the
//! executor; nothing else writes the state machine, so a handler cannot leave a
//! run stuck.

use base::error::AppError;
use serde::Serialize;

use super::spec::{JobKey, Visibility};

/// Identity of one execution, and the string a client polls with.
///
/// Random rather than sequential: it is the only thing protecting a run from
/// another account, so it must not be guessable.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct RunId(String);

impl RunId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Rebuild an id a client supplied. The value is only ever used as a map
    /// key, so an unparseable one simply misses.
    pub fn from_client(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a run is in its lifecycle.
///
/// `Yielded` is distinct from `Running`: it stepped aside for load but is still
/// alive, and no recovery pass may treat it as dead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Running,
    /// Parked at a checkpoint because the server was busy. Still active.
    Yielded,
    Succeeded,
    Failed(String),
    Cancelled,
    TimedOut,
    /// The server restarted (or crashed) while this run was active.
    Interrupted,
}

impl JobState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Failed(_)
                | Self::Cancelled
                | Self::TimedOut
                | Self::Interrupted
        )
    }

    /// Whether the run still occupies a concurrency slot.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Yielded)
    }

    /// The state a stored row means; [`JobState::as_str`] is the inverse, read
    /// back out of the journal after an in-place restart empties the counters.
    /// An unknown phase reads as a failure rather than being dropped.
    pub fn from_wire(phase: &str, error: Option<&str>) -> Self {
        match phase {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "yielded" => Self::Yielded,
            "succeeded" => Self::Succeeded,
            "cancelled" => Self::Cancelled,
            "timed_out" => Self::TimedOut,
            "interrupted" => Self::Interrupted,
            _ => Self::Failed(error.unwrap_or_default().to_string()),
        }
    }

    /// Stable wire name, used by the client-compatibility projections.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Yielded => "yielded",
            Self::Succeeded => "succeeded",
            Self::Failed(_) => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Interrupted => "interrupted",
        }
    }
}

/// How far along a run is; `total` is optional because not every job can say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: Option<u64>,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub message: String,
    pub processed: Option<u64>,
    /// The body found nothing to do.
    ///
    /// A scheduled pass that found nothing leaves no trace; a manual trigger or
    /// a request is always recorded, as is a failure.
    pub idle: bool,
}

impl Outcome {
    pub fn success(message: impl Into<String>, processed: Option<u64>) -> Self {
        Self {
            message: message.into(),
            processed,
            idle: false,
        }
    }

    /// A pass that found nothing to do.
    pub fn idle(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            processed: None,
            idle: true,
        }
    }

    pub fn ok() -> Self {
        Self::default()
    }
}

/// Why a job did not succeed. Cancellation and timeout are first-class so the
/// executor can map them onto a terminal state without parsing text.
#[derive(Debug)]
pub enum JobFailure {
    App(AppError),
    Cancelled,
    TimedOut,
}

impl From<AppError> for JobFailure {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl JobFailure {
    /// The terminal state this failure produces.
    pub fn into_state(self) -> JobState {
        match self {
            Self::Cancelled => JobState::Cancelled,
            Self::TimedOut => JobState::TimedOut,
            Self::App(e) => JobState::Failed(e.to_string()),
        }
    }
}

/// Who asked for a run; it decides what an idle run leaves behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A periodic timer. Nobody is waiting for it and its next tick re-does
    /// whatever this one would have done, so an idle pass leaves no history.
    Schedule,
    /// A request handler, for a client that polls the id.
    Request,
    /// An administrator pressing "Run now". The record is the answer to the
    /// press, so it is kept even when the job found nothing to do.
    Operator,
    /// A one-shot pass the server starts itself: recovery, or the startup
    /// conversion. Kept like `Operator` — nobody can press it again.
    Startup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Viewer {
    Anonymous,
    User { id: i32, is_admin: bool },
}

impl Viewer {
    pub fn user(id: i32) -> Self {
        Self::User {
            id,
            is_admin: false,
        }
    }

    pub fn admin(id: i32) -> Self {
        Self::User { id, is_admin: true }
    }
}

/// One execution of one job.
#[derive(Clone, Debug)]
pub struct JobRun {
    pub id: RunId,
    pub key: JobKey,
    pub origin: Origin,
    /// Denormalized from the spec so the store can filter without the registry.
    pub visibility: Visibility,
    /// `None` for a system job.
    pub owner: Option<i32>,
    pub state: JobState,
    pub progress: Progress,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// 1 for the first execution; incremented by each retry.
    pub attempt: u32,
    /// The submit-time input, dropped once the run reaches a terminal state;
    /// `expected_total` and `summary` keep what a wire projection still needs.
    pub params: Option<Params>,
    /// `total` as the submitting handler knew it, kept past the params drop.
    pub expected_total: Option<u64>,
    /// Human summary, also kept past the params drop.
    pub summary: String,
    /// Small job-specific facts a client-compatibility projection still needs
    /// after the params are gone (a reindex run's repository, its indexed and
    /// skipped counts). Bounded by [`MAX_DETAIL_KEYS`].
    pub details: serde_json::Map<String, serde_json::Value>,
}

/// Upper bound on [`JobRun::details`] entries.
pub const MAX_DETAIL_KEYS: usize = 16;

/// A job's input, opaque to the task system.
pub type Params = serde_json::Value;

impl JobRun {
    /// Build a run in `Queued`.
    #[allow(clippy::too_many_arguments)]
    pub fn queued(
        key: JobKey,
        origin: Origin,
        visibility: Visibility,
        owner: Option<i32>,
        params: Params,
        summary: impl Into<String>,
        expected_total: Option<u64>,
        now: i64,
    ) -> Self {
        Self {
            id: RunId::new(),
            key,
            origin,
            visibility,
            owner,
            state: JobState::Queued,
            progress: Progress::default(),
            created_at: now,
            started_at: None,
            finished_at: None,
            attempt: 1,
            params: Some(params),
            expected_total,
            summary: summary.into(),
            details: serde_json::Map::new(),
        }
    }

    /// Record a small terminal fact for the wire projection. Silently ignored
    /// past [`MAX_DETAIL_KEYS`], so a job cannot turn this into a payload store.
    pub fn set_detail(&mut self, key: &str, value: serde_json::Value) {
        if self.details.contains_key(key) || self.details.len() < MAX_DETAIL_KEYS {
            self.details.insert(key.to_string(), value);
        }
    }

    /// Whether this run may be shown to `viewer`.
    ///
    /// Another account's run reads as missing rather than forbidden, so the answer
    /// cannot probe which ids exist. [`Visibility::Owner`] has no administrator
    /// override: that payload carries file names.
    pub fn is_visible_to(&self, viewer: Viewer) -> bool {
        match self.visibility {
            Visibility::Admin => match viewer {
                Viewer::Anonymous => false,
                Viewer::User { is_admin, .. } => is_admin,
            },
            Visibility::OwnerOrAdmin => match viewer {
                Viewer::Anonymous => false,
                Viewer::User { id, is_admin } => is_admin || self.owner == Some(id),
            },
            Visibility::Owner => match viewer {
                Viewer::Anonymous => false,
                Viewer::User { id, .. } => self.owner == Some(id),
            },
        }
    }

    /// Approximate heap footprint, for the store's byte budget.
    pub fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.id.as_str().len()
            + self.summary.len()
            + self.progress.message.len()
            + match &self.state {
                JobState::Failed(e) => e.len(),
                _ => 0,
            }
            + self.params.as_ref().map_or(0, |p| p.to_string().len())
            + self
                .details
                .iter()
                .map(|(k, v)| k.len() + v.to_string().len())
                .sum::<usize>()
    }

    /// Release the submit-time input once the run can no longer need it; a wire
    /// projection reads `expected_total`/`summary` instead.
    pub fn drop_params(&mut self) {
        self.params = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(visibility: Visibility, owner: Option<i32>) -> JobRun {
        JobRun::queued(
            JobKey::Copy,
            Origin::Request,
            visibility,
            owner,
            serde_json::json!({"src_dirents": ["a", "b"]}),
            "Copy 2 items",
            Some(2),
            0,
        )
    }

    /// An idle verdict is opt-in: a body that does not say so did work.
    #[test]
    fn only_an_idle_outcome_says_it_found_nothing() {
        assert!(!Outcome::success("did it", Some(1)).idle);
        assert!(!Outcome::ok().idle);
        assert!(Outcome::idle("nothing to do").idle);
        assert_eq!(Outcome::idle("nothing to do").message, "nothing to do");
        assert_eq!(Outcome::idle("nothing to do").processed, None);
    }

    #[test]
    fn terminal_and_active_states_are_disjoint() {
        for state in [
            JobState::Succeeded,
            JobState::Failed("x".into()),
            JobState::Cancelled,
            JobState::TimedOut,
            JobState::Interrupted,
        ] {
            assert!(state.is_terminal(), "{state:?} is terminal");
            assert!(!state.is_active(), "{state:?} is not active");
        }
        for state in [JobState::Queued, JobState::Running, JobState::Yielded] {
            assert!(!state.is_terminal(), "{state:?} is not terminal");
            assert!(state.is_active(), "{state:?} is active");
        }
    }

    /// A yielded run is still alive, so it must not be treated as a dead one.
    #[test]
    fn yielded_is_active_not_terminal() {
        assert!(JobState::Yielded.is_active());
        assert!(!JobState::Yielded.is_terminal());
    }

    #[test]
    fn owner_visibility_hides_other_users_runs() {
        let r = run(Visibility::Owner, Some(7));
        assert!(r.is_visible_to(Viewer::user(7)));
        assert!(!r.is_visible_to(Viewer::user(8)));
        assert!(!r.is_visible_to(Viewer::Anonymous));
        assert!(!r.is_visible_to(Viewer::admin(9)));
    }

    #[test]
    fn owner_or_admin_visibility() {
        let r = run(Visibility::OwnerOrAdmin, Some(7));
        assert!(r.is_visible_to(Viewer::user(7)));
        assert!(!r.is_visible_to(Viewer::user(8)));
        assert!(r.is_visible_to(Viewer::admin(8)));
    }

    #[test]
    fn admin_visibility_is_admin_only() {
        let r = run(Visibility::Admin, Some(7));
        assert!(!r.is_visible_to(Viewer::user(7)));
        assert!(r.is_visible_to(Viewer::admin(7)));
    }

    #[test]
    fn drop_params_keeps_what_a_projection_needs() {
        let mut r = run(Visibility::Owner, Some(1));
        assert!(r.bytes() > std::mem::size_of::<JobRun>());
        r.drop_params();
        assert!(r.params.is_none());
        assert_eq!(r.expected_total, Some(2));
        assert_eq!(r.summary, "Copy 2 items");
    }

    #[test]
    fn ids_are_unique_and_displayable() {
        let a = RunId::new();
        let b = RunId::new();
        assert_ne!(a, b);
        assert_eq!(a.to_string(), a.as_str());
        assert_eq!(RunId::from_client("x").as_str(), "x");
    }
}
