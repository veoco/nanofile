//! The set of registered jobs: one policy from [`super::catalog`] paired with
//! the body that implements it for the current server generation.
//!
//! Registration binds the two, which is also when the policy invariants are
//! checked. A mis-declared job — a `QuietPolicy` with no checkpoint to act on, a
//! durable job that is not idempotent — is refused here, at startup, rather than
//! discovered later as a job that silently never defers or that corrupts data
//! after a crash.

use std::collections::HashMap;
use std::sync::Arc;

use super::spec::{JobKey, JobRunFn, JobSpec, SpecError};

/// One job: what it is, and how to run it.
pub struct RegisteredJob {
    pub spec: JobSpec,
    pub run: JobRunFn,
}

impl RegisteredJob {
    pub fn new(spec: JobSpec, run: JobRunFn) -> Self {
        Self { spec, run }
    }

    pub fn key(&self) -> JobKey {
        self.spec.key
    }

    pub fn name(&self) -> &'static str {
        self.spec.name
    }
}

impl std::fmt::Debug for RegisteredJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredJob")
            .field("key", &self.spec.key)
            .field("name", &self.spec.name)
            .finish_non_exhaustive()
    }
}

/// Why a job could not be registered.
#[derive(Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// The policy violates an invariant.
    Spec(SpecError),
    /// Two jobs claim the same key. A programming error rather than a runtime
    /// condition: the old scheduler silently overwrote in one container and
    /// appended in another, which produced two admin rows for one task.
    Duplicate(JobKey),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spec(e) => write!(f, "{e}"),
            Self::Duplicate(key) => write!(f, "job {key:?} is registered twice"),
        }
    }
}

impl From<SpecError> for RegistryError {
    fn from(value: SpecError) -> Self {
        Self::Spec(value)
    }
}

/// Every registered job, addressable by key.
#[derive(Default)]
pub struct JobRegistry {
    jobs: HashMap<JobKey, Arc<RegisteredJob>>,
    /// Registration order, so the admin listing is stable rather than
    /// hash-ordered.
    order: Vec<JobKey>,
}

impl std::fmt::Debug for JobRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRegistry")
            .field("jobs", &self.order)
            .finish()
    }
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate and add a job.
    pub fn register(&mut self, job: RegisteredJob) -> Result<(), RegistryError> {
        let key = job.key();
        if self.jobs.contains_key(&key) {
            return Err(RegistryError::Duplicate(key));
        }
        job.spec.validate()?;
        self.jobs.insert(key, Arc::new(job));
        self.order.push(key);
        Ok(())
    }

    pub fn get(&self, key: JobKey) -> Option<&Arc<RegisteredJob>> {
        self.jobs.get(&key)
    }

    /// Every job, in registration order.
    pub fn jobs(&self) -> Vec<&Arc<RegisteredJob>> {
        self.order.iter().filter_map(|k| self.jobs.get(k)).collect()
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::spec::{
        ChunkPolicy, Dedup, Durability, Priority, QuietPolicy, Resource, Retention, RetryPolicy,
        SpikePolicy, TimeoutPolicy, Trigger, Visibility,
    };

    fn policy(key: JobKey) -> JobSpec {
        JobSpec {
            key,
            name: "test",
            trigger: Trigger::Manual,
            priority: Priority::Normal,
            resource: Resource::Cpu,
            max_concurrent: 1,
            queue_depth: 0,
            timeout: TimeoutPolicy::default(),
            retry: RetryPolicy::Never,
            dedup: Dedup::None,
            retention: Retention::default(),
            visibility: Visibility::Owner,
            durability: Durability::Memory,
            resumable: true,
            cancellable: true,
            chunkable: None,
            force_safe: false,
            quiet: None,
            idempotent: true,
        }
    }

    fn job(key: JobKey) -> RegisteredJob {
        RegisteredJob::new(
            policy(key),
            Arc::new(|_ctx, _params| Box::pin(async { Ok(super::super::run::Outcome::ok()) })),
        )
    }

    #[test]
    fn registers_and_looks_up_by_key() {
        let mut registry = JobRegistry::new();
        registry.register(job(JobKey::Copy)).unwrap();
        registry.register(job(JobKey::Reindex)).unwrap();
        assert_eq!(registry.len(), 2);
        assert!(registry.get(JobKey::Copy).is_some());
        assert!(registry.get(JobKey::Move).is_none());
    }

    #[test]
    fn listing_follows_registration_order() {
        let mut registry = JobRegistry::new();
        registry.register(job(JobKey::Reindex)).unwrap();
        registry.register(job(JobKey::Copy)).unwrap();
        registry.register(job(JobKey::Move)).unwrap();
        let order: Vec<JobKey> = registry.jobs().iter().map(|j| j.key()).collect();
        assert_eq!(order, vec![JobKey::Reindex, JobKey::Copy, JobKey::Move]);
    }

    #[test]
    fn a_duplicate_key_is_refused_rather_than_overwriting() {
        let mut registry = JobRegistry::new();
        registry.register(job(JobKey::Copy)).unwrap();
        assert_eq!(
            registry.register(job(JobKey::Copy)),
            Err(RegistryError::Duplicate(JobKey::Copy))
        );
        assert_eq!(registry.len(), 1, "the original registration stands");
    }

    #[test]
    fn an_invalid_policy_is_refused() {
        let mut registry = JobRegistry::new();
        let mut bad = job(JobKey::GarbageCollection);
        bad.spec.priority = Priority::Background;
        bad.spec.quiet = Some(QuietPolicy {
            min_idle_for_secs: 30,
            max_deferral_hours: 24,
            on_spike: SpikePolicy::Yield,
        });
        // No chunk policy, so the quiet policy could never act.
        assert_eq!(
            registry.register(bad),
            Err(RegistryError::Spec(SpecError::DeferrableWithoutChunks(
                JobKey::GarbageCollection
            )))
        );

        let mut good = job(JobKey::GarbageCollection);
        good.spec.priority = Priority::Background;
        good.spec.chunkable = Some(ChunkPolicy {
            max_chunk_ms: 200,
            unit: "repo",
        });
        good.spec.quiet = Some(QuietPolicy {
            min_idle_for_secs: 30,
            max_deferral_hours: 24,
            on_spike: SpikePolicy::Yield,
        });
        assert!(registry.register(good).is_ok());
    }
}
