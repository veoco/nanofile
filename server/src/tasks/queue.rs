//! The scheduling policy a persistent work queue runs on.
//!
//! Extracted from the outbound-mail queue, which is the one persistent queue in
//! the codebase and therefore the one whose behaviour is known to work. The
//! claim and lease mechanics stay in the repository that owns the table — an
//! abstraction with a single implementation would only add indirection — but
//! the numbers and the shape of the schedule are shared here, so a second queue
//! (the job history table) cannot invent a different backoff or forget to lease.

/// Exponential backoff with a ceiling, the shape both queues use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueuePolicy {
    pub base_backoff_secs: u64,
    pub max_backoff_secs: u64,
    /// How long a claim owns a row before another worker may take it over.
    ///
    /// Long enough to cover a slow delivery, short enough that a worker that
    /// died does not strand the row.
    pub lease_secs: i64,
    /// Finished rows kept by age.
    pub sent_retention_secs: i64,
    pub failed_retention_secs: i64,
    /// Finished rows kept by count, oldest dropped first.
    pub max_finished_rows: u64,
}

impl QueuePolicy {
    /// The policy the outbound-mail queue has always used, and the default for
    /// any other queue.
    pub const DEFAULT: Self = Self {
        base_backoff_secs: 30,
        max_backoff_secs: 3600,
        lease_secs: 300,
        sent_retention_secs: 30 * 86_400,
        failed_retention_secs: 90 * 86_400,
        max_finished_rows: 1000,
    };
}

impl Default for QueuePolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl QueuePolicy {
    /// Delay before the next attempt after `attempts` have been made.
    pub fn backoff_secs(&self, attempts: i32) -> i64 {
        // Clamped so a row that has somehow accumulated a huge attempt count
        // cannot overflow the shift.
        let exponent = attempts.clamp(1, 32).saturating_sub(1).min(20);
        let factor = 1i64 << exponent;
        (self.base_backoff_secs as i64)
            .saturating_mul(factor)
            .min(self.max_backoff_secs as i64)
    }

    /// When attempt number `attempts + 1` becomes due.
    pub fn next_attempt_at(&self, now: i64, attempts: i32) -> i64 {
        now.saturating_add(self.backoff_secs(attempts))
    }

    /// When a claim taken at `now` stops owning the row.
    pub fn lease_until(&self, now: i64) -> i64 {
        now.saturating_add(self.lease_secs)
    }

    /// The retention cutoffs for a prune at `now`.
    pub fn retention_cutoffs(&self, now: i64) -> (i64, i64) {
        (
            now.saturating_sub(self.sent_retention_secs),
            now.saturating_sub(self.failed_retention_secs),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_exponential_from_the_base() {
        let p = QueuePolicy::default();
        assert_eq!(p.backoff_secs(1), 30);
        assert_eq!(p.backoff_secs(2), 60);
        assert_eq!(p.backoff_secs(3), 120);
        assert_eq!(p.backoff_secs(4), 240);
    }

    #[test]
    fn backoff_is_capped() {
        let p = QueuePolicy::default();
        assert_eq!(p.backoff_secs(8), 3600, "30 * 2^7 exceeds the ceiling");
        assert_eq!(p.backoff_secs(32), 3600);
        assert_eq!(p.backoff_secs(10_000), 3600, "a wild count cannot overflow");
        assert_eq!(p.backoff_secs(0), 30, "a zero count behaves like the first");
    }

    #[test]
    fn a_lease_runs_from_the_claim() {
        let p = QueuePolicy::default();
        assert_eq!(p.lease_until(1_000), 1_300);
        assert_eq!(p.lease_until(i64::MAX), i64::MAX, "saturates");
    }

    #[test]
    fn retention_cutoffs_are_derived_from_now() {
        let p = QueuePolicy::default();
        let (sent, failed) = p.retention_cutoffs(1_000_000_000);
        assert_eq!(sent, 1_000_000_000 - 30 * 86_400);
        assert_eq!(failed, 1_000_000_000 - 90 * 86_400);
        assert!(sent > failed, "sent rows are dropped sooner");
    }

    #[test]
    fn next_attempt_uses_the_backoff() {
        let p = QueuePolicy::default();
        assert_eq!(p.next_attempt_at(100, 1), 130);
        assert_eq!(p.next_attempt_at(i64::MAX, 3), i64::MAX);
    }
}
