//! The outbox: queueing a rendered message and getting it delivered, with
//! retries that survive a restart.
//!
//! Delivery is **at-least-once**. A row is claimed by incrementing `attempts`
//! in a conditional `UPDATE`, so a process that dies mid-send leaves the row
//! pending and it will be tried again; the cost is a possible duplicate, which
//! for a notification or a reset link is harmless. Exactly-once would need a
//! distributed lock, and would still be defeated by a relay that accepted the
//! message and then lost it.

use base::error::AppError;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use infra::crypto::token_encryption::TokenCipher;
use infra::entity::email_message;

use crate::repository::Repositories;
use crate::repository::email_message::NewEmailMessage;

use super::settings::EmailSettings;
use super::transport::MailTransport;

/// Messages attempted per drain. Bounds how long one scheduler tick can run
/// when a long outage has piled messages up.
pub const DRAIN_BATCH: u64 = 25;

/// The queue's schedule: backoff, claim lease and retention. Shared with any
/// other persistent queue rather than restated here.
pub const POLICY: crate::tasks::queue::QueuePolicy = crate::tasks::queue::QueuePolicy::DEFAULT;

/// Delay before the next attempt after `attempts` failed tries.
///
/// 30s, 1m, 2m, 4m … capped at an hour: long enough that a down relay is not
/// hammered, short enough that a transient failure is invisible to the user.
pub fn backoff_secs(attempts: i32) -> i64 {
    POLICY.backoff_secs(attempts)
}

/// When the next attempt is due after `attempts` failed tries.
pub fn next_attempt_at(now: i64, attempts: i32) -> i64 {
    POLICY.next_attempt_at(now, attempts)
}

/// Encode a rendered message for storage.
///
/// The raw bytes go through base64 before encryption because the column is
/// text: a message is mostly ASCII (quoted-printable/base64 bodies, encoded
/// headers), but a header holding raw UTF-8 must not be able to break the
/// round-trip.
pub fn encode_body(raw: &[u8]) -> String {
    BASE64.encode(raw)
}

/// Inverse of [`encode_body`].
pub fn decode_body(encoded: &str) -> Option<Vec<u8>> {
    BASE64.decode(encoded).ok()
}

/// What one drain accomplished, for the scheduler's task metrics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub delivered: u64,
    pub retried: u64,
    pub failed: u64,
    pub pruned: u64,
}

impl DrainReport {
    /// A one-line summary for `/sysadmin/tasks/`.
    pub fn summary(&self) -> String {
        format!(
            "{} delivered, {} retrying, {} failed, {} pruned",
            self.delivered, self.retried, self.failed, self.pruned
        )
    }

    pub fn attempted(&self) -> u64 {
        self.delivered + self.retried + self.failed
    }
}

/// Queue a rendered message. Returns the row so the caller can attempt it.
pub async fn enqueue(
    repos: &Repositories,
    cipher: &TokenCipher,
    kind: &str,
    to: &str,
    user_id: Option<i32>,
    subject: &str,
    raw: &[u8],
    now: i64,
) -> Result<email_message::Model, AppError> {
    repos
        .email_message
        .insert(NewEmailMessage {
            kind: kind.to_string(),
            to_address: to.trim().to_string(),
            user_id,
            subject: subject.to_string(),
            body_enc: cipher.encrypt(&encode_body(raw)),
            created_at: now,
        })
        .await
}

/// Try to deliver one queued message.
///
/// `Ok(true)` means it was delivered. `Ok(false)` covers every other outcome —
/// another attempt claimed it, the body is unreadable, or SMTP refused — with
/// the row already updated to say which. Errors are reserved for database
/// failures, which the caller should log rather than retry blindly.
pub async fn attempt(
    repos: &Repositories,
    cipher: &TokenCipher,
    settings: &EmailSettings,
    hello_name: &str,
    row: &email_message::Model,
    now: i64,
) -> Result<bool, AppError> {
    // Claiming is what makes a crash mid-send safe, and the lease is what stops
    // a second drain (or a manual retry) from sending the same body twice: the
    // attempt counter alone is not exclusive, because a drain that reads the row
    // after the first claim commits sees the incremented value and its own swap
    // succeeds.
    if !repos
        .email_message
        .claim(row.id, row.attempts, now, POLICY.lease_until(now))
        .await?
    {
        return Ok(false);
    }
    let attempt_no = row.attempts + 1;

    let raw = row
        .body_enc
        .as_deref()
        .and_then(|stored| cipher.decrypt(stored))
        .and_then(|encoded| decode_body(&encoded));
    let Some(raw) = raw else {
        // Give up immediately rather than retrying: no amount of waiting makes
        // an unreadable body readable (the usual cause is a rotated secret).
        repos
            .email_message
            .mark_attempt_failed(
                row.id,
                None,
                "the queued message body is missing or cannot be decrypted; \
                 this usually means the server secret_key changed",
            )
            .await?;
        return Ok(false);
    };

    let failure = match MailTransport::new(settings, hello_name) {
        Ok(transport) => transport
            .send_raw(&settings.from_address, &row.to_address, raw)
            .await
            .err()
            .map(|e| e.to_string()),
        Err(e) => Some(e.to_string()),
    };

    match failure {
        None => {
            repos.email_message.mark_sent(row.id, now).await?;
            Ok(true)
        }
        Some(error) => {
            let next = (attempt_no < settings.max_attempts as i32)
                .then(|| next_attempt_at(now, attempt_no));
            repos
                .email_message
                .mark_attempt_failed(row.id, next, &error)
                .await?;
            Ok(false)
        }
    }
}

/// Deliver everything that is due, then apply retention.
pub async fn drain(
    repos: &Repositories,
    cipher: &TokenCipher,
    settings: &EmailSettings,
    hello_name: &str,
    now: i64,
) -> Result<DrainReport, AppError> {
    let mut report = DrainReport::default();
    // A lease whose holder died must not strand its row: clear it before the
    // due query so this drain can pick the message up.
    reap_leases(repos, now).await?;
    for row in repos.email_message.due_pending(now, DRAIN_BATCH).await? {
        let attempts_before = row.attempts;
        if attempt(repos, cipher, settings, hello_name, &row, now).await? {
            report.delivered += 1;
        } else {
            // Re-read the row rather than guessing: `attempt` may have been
            // beaten to the claim by a manual retry, in which case the outcome
            // is neither a retry nor a terminal failure.
            match repos.email_message.find_by_id(row.id).await? {
                Some(after) if after.status == email_message::Status::Failed.id() => {
                    report.failed += 1;
                }
                Some(after) if after.attempts > attempts_before => {
                    report.retried += 1;
                }
                _ => {}
            }
        }
    }
    report.pruned = prune(repos, now).await?;
    Ok(report)
}

/// Clear leases whose holder disappeared, so their rows become claimable.
///
/// A row is left pending with its attempt counted when the process dies
/// mid-send; clearing the lease is all that is needed for the next drain to
/// pick it up. Called before each drain rather than on its own timer.
pub async fn reap_leases(repos: &Repositories, now: i64) -> Result<u64, AppError> {
    repos.email_message.reap_expired_leases(now).await
}

/// Apply the retention policy, returning how many rows were removed.
pub async fn prune(repos: &Repositories, now: i64) -> Result<u64, AppError> {
    repos
        .email_message
        .prune(
            POLICY.retention_cutoffs(now).0,
            POLICY.retention_cutoffs(now).1,
            POLICY.max_finished_rows,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use infra::entity::email_message::Status;
    use migration::MigratorTrait;
    use sea_orm::Database;
    use std::sync::Arc;

    async fn repos() -> Arc<Repositories> {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        migration::Migrator::up(&db, None).await.unwrap();
        Arc::new(Repositories::new_for_tests(Arc::new(db)))
    }

    fn cipher() -> TokenCipher {
        TokenCipher::from_master_key(b"test-secret")
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_secs(1), 30);
        assert_eq!(backoff_secs(2), 60);
        assert_eq!(backoff_secs(3), 120);
        assert_eq!(backoff_secs(20), POLICY.max_backoff_secs as i64);
        // A nonsensical attempt number must not shift by a negative amount.
        assert_eq!(backoff_secs(0), 30);
        assert_eq!(backoff_secs(-5), 30);
        assert_eq!(backoff_secs(i32::MAX), POLICY.max_backoff_secs as i64);
        assert!(backoff_secs(8) > backoff_secs(7));
    }

    #[test]
    fn the_body_round_trips_through_base64_and_encryption() {
        let raw = "Subject: 测试\r\n\r\nbody with \u{1F600} and \r\n.\r\n".as_bytes();
        let stored = cipher().encrypt(&encode_body(raw));
        let decoded = decode_body(&cipher().decrypt(&stored).unwrap()).unwrap();
        assert_eq!(decoded, raw);
        assert!(decode_body("not base64 !!!").is_none());
    }

    #[tokio::test]
    async fn a_queued_row_is_due_immediately_and_unattempted() {
        let repos = repos().await;
        let row = enqueue(
            &repos,
            &cipher(),
            "new_login",
            "user@example.com",
            Some(7),
            "hello",
            b"raw",
            100,
        )
        .await
        .unwrap();

        assert_eq!(row.status, Status::Pending.id());
        assert_eq!(row.attempts, 0);
        assert_eq!(row.next_attempt_at, 100);
        assert_eq!(row.user_id, Some(7));
        assert!(!row.body_enc.as_deref().unwrap().contains("raw"));

        let due = repos.email_message.due_pending(100, 10).await.unwrap();
        assert_eq!(due.len(), 1);
        assert!(
            repos
                .email_message
                .due_pending(99, 10)
                .await
                .unwrap()
                .is_empty(),
            "a row is not due before its deadline"
        );
    }

    #[tokio::test]
    async fn claiming_increments_attempts_and_only_once() {
        let repos = repos().await;
        let row = enqueue(
            &repos,
            &cipher(),
            "test",
            "a@example.com",
            None,
            "s",
            b"raw",
            0,
        )
        .await
        .unwrap();

        assert!(
            repos
                .email_message
                .claim(row.id, 0, 0, POLICY.lease_until(0))
                .await
                .unwrap()
        );
        assert!(
            !repos
                .email_message
                .claim(row.id, 0, 0, POLICY.lease_until(0))
                .await
                .unwrap(),
            "a claimed row cannot be claimed again by a caller holding the old count"
        );
        let after = repos
            .email_message
            .find_by_id(row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.attempts, 1);
    }

    /// The bug the lease exists for: the attempt counter alone is not
    /// exclusive. A drain that re-reads the row after the first claim commits
    /// sees the incremented counter, so its own compare-and-swap succeeds and
    /// both send. The lease closes that window.
    #[tokio::test]
    async fn a_second_claim_with_the_fresh_attempt_count_is_still_refused() {
        let repos = repos().await;
        let row = enqueue(
            &repos,
            &cipher(),
            "new_login",
            "a@example.com",
            None,
            "s",
            b"raw",
            0,
        )
        .await
        .unwrap();

        assert!(
            repos
                .email_message
                .claim(row.id, 0, 0, POLICY.lease_until(0))
                .await
                .unwrap()
        );
        // Exactly the case the old conditional update allowed through: a second
        // worker that read `attempts = 1` after the first claim landed.
        let reread = repos
            .email_message
            .find_by_id(row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reread.attempts, 1);
        assert!(
            !repos
                .email_message
                .claim(row.id, reread.attempts, 0, POLICY.lease_until(0))
                .await
                .unwrap(),
            "the first claim's lease must keep a second drain out"
        );
    }

    /// A lease whose holder died must not strand the row.
    #[tokio::test]
    async fn an_expired_lease_is_reaped_and_the_row_becomes_claimable() {
        let repos = repos().await;
        let row = enqueue(
            &repos,
            &cipher(),
            "new_login",
            "a@example.com",
            None,
            "s",
            b"raw",
            0,
        )
        .await
        .unwrap();

        assert!(
            repos
                .email_message
                .claim(row.id, 0, 0, POLICY.lease_until(0))
                .await
                .unwrap()
        );
        assert_eq!(reap_leases(&repos, 0).await.unwrap(), 0, "still leased");

        let after_expiry = POLICY.lease_secs + 1;
        assert_eq!(reap_leases(&repos, after_expiry).await.unwrap(), 1);
        let reread = repos
            .email_message
            .find_by_id(row.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            repos
                .email_message
                .claim(
                    row.id,
                    reread.attempts,
                    after_expiry,
                    POLICY.lease_until(after_expiry)
                )
                .await
                .unwrap(),
            "once the lease is reaped the row is claimable again"
        );
    }

    /// A lease still held keeps its row out of a competing drain even after the
    /// counter moved, and the same drain can extend it.
    #[tokio::test]
    async fn a_lease_can_be_renewed() {
        let repos = repos().await;
        let row = enqueue(
            &repos,
            &cipher(),
            "new_login",
            "a@example.com",
            None,
            "s",
            b"raw",
            0,
        )
        .await
        .unwrap();
        assert!(
            repos
                .email_message
                .claim(row.id, 0, 0, POLICY.lease_until(0))
                .await
                .unwrap()
        );

        let renewed = POLICY.lease_until(1_000);
        repos
            .email_message
            .renew_lease(row.id, renewed)
            .await
            .unwrap();
        assert_eq!(
            repos
                .email_message
                .find_by_id(row.id)
                .await
                .unwrap()
                .unwrap()
                .lease_until,
            Some(renewed)
        );
        assert_eq!(
            reap_leases(&repos, 1_000).await.unwrap(),
            0,
            "the renewed lease still holds"
        );
    }

    #[tokio::test]
    async fn an_unreadable_body_fails_immediately_with_the_reason() {
        let repos = repos().await;
        let row = enqueue(
            &repos,
            &cipher(),
            "test",
            "a@example.com",
            None,
            "s",
            b"raw",
            0,
        )
        .await
        .unwrap();

        // The row decrypts with the same secret it was encrypted with; a
        // different secret is what an operator sees after rotating it.
        let settings = super::super::settings::EmailSettings::from_config(&Default::default());
        let delivered = attempt(
            &repos,
            &TokenCipher::from_master_key(b"another-secret"),
            &settings,
            "localhost",
            &row,
            0,
        )
        .await
        .unwrap();
        assert!(!delivered);

        let after = repos
            .email_message
            .find_by_id(row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, Status::Failed.id());
        assert_eq!(after.attempts, 1);
        assert!(
            after.last_error.as_deref().unwrap().contains("decrypt"),
            "the reason has to name the actual problem: {:?}",
            after.last_error
        );
    }

    #[tokio::test]
    async fn retention_prunes_by_age_and_by_size() {
        let repos = repos().await;
        let now = 1_700_000_000i64;
        let cipher = cipher();

        // An old delivered row and an old failed row, each past its own
        // retention window (30 days for delivered mail, 90 for failures).
        for (age, kind) in [
            (POLICY.sent_retention_secs + 10_000, "old_sent"),
            (POLICY.failed_retention_secs + 10_000, "old_failed"),
        ] {
            let row = enqueue(
                &repos,
                &cipher,
                kind,
                "a@example.com",
                None,
                "s",
                b"raw",
                now - age,
            )
            .await
            .unwrap();
            let status = if kind == "old_sent" {
                Status::Sent
            } else {
                Status::Failed
            };
            repos
                .email_message
                .mark_attempt_failed(row.id, None, "gone")
                .await
                .unwrap();
            if status == Status::Sent {
                repos.email_message.mark_sent(row.id, now).await.unwrap();
            }
        }
        // A pending row is never pruned by age.
        let pending = enqueue(
            &repos,
            &cipher,
            "pending",
            "a@example.com",
            None,
            "s",
            b"raw",
            0,
        )
        .await
        .unwrap();

        let pruned = prune(&repos, now).await.unwrap();
        assert_eq!(pruned, 2, "both finished rows are past retention");
        assert!(
            repos
                .email_message
                .find_by_id(pending.id)
                .await
                .unwrap()
                .is_some(),
            "a pending row is never pruned by age"
        );
    }

    #[tokio::test]
    async fn a_drained_message_that_cannot_reach_a_server_is_retried_then_failed() {
        let repos = repos().await;
        let cipher = cipher();
        // Nothing listens on this port, so every attempt fails to connect.
        let mut settings = super::super::settings::EmailSettings::from_config(&Default::default());
        settings.enabled = true;
        settings.host = "127.0.0.1".to_string();
        settings.port = 1;
        settings.from_address = "nanofile@example.com".to_string();
        settings.timeout_secs = 1;
        settings.max_attempts = 2;
        settings.tls = super::super::settings::TlsMode::None;

        let row = enqueue(
            &repos,
            &cipher,
            "test",
            "a@example.com",
            None,
            "s",
            b"raw",
            100,
        )
        .await
        .unwrap();

        let report = drain(&repos, &cipher, &settings, "localhost", 100)
            .await
            .unwrap();
        assert_eq!(report.retried, 1);
        let after = repos
            .email_message
            .find_by_id(row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.attempts, 1);
        assert!(after.next_attempt_at > 100, "the retry is delayed");
        assert!(after.last_error.is_some());

        // The second attempt is the last one allowed.
        let report = drain(
            &repos,
            &cipher,
            &settings,
            "localhost",
            after.next_attempt_at,
        )
        .await
        .unwrap();
        assert_eq!(report.failed, 1);
        let after = repos
            .email_message
            .find_by_id(row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, Status::Failed.id());
        assert_eq!(after.attempts, 2);
    }

    #[tokio::test]
    async fn a_failed_row_can_be_requeued_by_hand() {
        let repos = repos().await;
        let row = enqueue(
            &repos,
            &cipher(),
            "test",
            "a@example.com",
            None,
            "s",
            b"raw",
            0,
        )
        .await
        .unwrap();
        repos
            .email_message
            .mark_attempt_failed(row.id, None, "gave up")
            .await
            .unwrap();

        assert!(repos.email_message.requeue(row.id, 50).await.unwrap());
        let after = repos
            .email_message
            .find_by_id(row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, Status::Pending.id());
        assert_eq!(after.next_attempt_at, 50);
        assert!(after.last_error.is_none());
        // A row that is not failed cannot be requeued again.
        assert!(!repos.email_message.requeue(row.id, 60).await.unwrap());
    }
}
