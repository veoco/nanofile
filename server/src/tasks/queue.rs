//! The scheduling policies the two persistent queues run on: the outbound-mail
//! queue, and the journal of finished job runs.
//!
//! Extracted from the outbound-mail queue, which is the one persistent queue in
//! the codebase and therefore the one whose behaviour is known to work. The
//! claim and lease mechanics stay in the repository that owns the table — an
//! abstraction with a single implementation would only add indirection — but
//! the numbers live here, where the two can be read side by side.
//!
//! They are two policies rather than one because the two queues keep very
//! different things. A delivered message is a receipt somebody may need to find
//! months later; a finished run is a record of what the server did, and past its
//! window nobody reads it. Sharing mail's numbers gave job history 30 days of
//! rows it had no room for — the count cap did all the work, and a 30-second
//! timer filled it.

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
    /// The policy the outbound-mail queue has always used.
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

/// How long the journal of finished runs keeps its rows.
///
/// Separate from [`QueuePolicy`] because the numbers mean something different
/// there: a message that was delivered is a receipt, while a run is a record
/// that something happened. Sizes are chosen to hold weeks of a healthy
/// server's runs, which — now that an idle tick leaves no row — is a handful a
/// day rather than thousands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobHistoryPolicy {
    /// How long a successful run stays readable.
    pub succeeded_retention_secs: i64,
    /// How long a run that did not succeed stays readable. Longer, because a
    /// failure is what somebody comes back for.
    pub failed_retention_secs: i64,
    /// Finished rows kept, oldest dropped first. Never applies to a row whose
    /// run has not finished: that is somebody's pending work.
    pub max_rows: u64,
}

impl JobHistoryPolicy {
    pub const DEFAULT: Self = Self {
        succeeded_retention_secs: 30 * 86_400,
        failed_retention_secs: 90 * 86_400,
        max_rows: 5000,
    };

    /// The age cutoffs for a prune at `now`: a successful run older than the
    /// first, or any other finished run older than the second, is dropped.
    pub fn cutoffs(&self, now: i64) -> (i64, i64) {
        (
            now.saturating_sub(self.succeeded_retention_secs),
            now.saturating_sub(self.failed_retention_secs),
        )
    }
}

impl Default for JobHistoryPolicy {
    fn default() -> Self {
        Self::DEFAULT
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

    /// The journal's cutoffs come from its own policy, and they are not the
    /// mail queue's: a failed run outlives a successful one, and neither is
    /// measured in a receipt's terms.
    #[test]
    fn the_journal_has_its_own_retention() {
        let p = JobHistoryPolicy::DEFAULT;
        let (succeeded, failed) = p.cutoffs(1_000_000_000);
        assert_eq!(succeeded, 1_000_000_000 - 30 * 86_400);
        assert_eq!(failed, 1_000_000_000 - 90 * 86_400);
        assert!(succeeded > failed, "a failure is kept longer");
        assert_ne!(
            p.max_rows,
            QueuePolicy::DEFAULT.max_finished_rows,
            "the two queues do not share a size"
        );
        assert_eq!(p.cutoffs(i64::MIN).0, i64::MIN, "saturates at the minimum");
    }

    #[test]
    fn next_attempt_uses_the_backoff() {
        let p = QueuePolicy::default();
        assert_eq!(p.next_attempt_at(100, 1), 130);
        assert_eq!(p.next_attempt_at(i64::MAX, 3), i64::MAX);
    }
}
