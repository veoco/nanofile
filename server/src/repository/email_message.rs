use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set,
};
use std::collections::HashMap;
use std::sync::Arc;

use base::error::AppError;
use infra::entity::email_message::{self, Status};

/// Parameters for queueing one message.
#[derive(Clone, Debug)]
pub struct NewEmailMessage {
    pub kind: String,
    pub to_address: String,
    pub user_id: Option<i32>,
    pub subject: String,
    /// Ciphertext of the rendered body — never the plaintext.
    pub body_enc: String,
    pub created_at: i64,
}

#[async_trait]
pub trait EmailMessageRepository: Send + Sync {
    async fn insert(&self, message: NewEmailMessage) -> Result<email_message::Model, AppError>;
    async fn find_by_id(&self, id: i32) -> Result<Option<email_message::Model>, AppError>;
    /// Newest first, optionally restricted to one status.
    async fn list_recent(
        &self,
        limit: u64,
        status: Option<Status>,
    ) -> Result<Vec<email_message::Model>, AppError>;
    /// One page of the outbox, newest first.
    async fn list_recent_page(
        &self,
        status: Option<Status>,
        limit: u64,
        offset: u64,
    ) -> Result<Vec<email_message::Model>, AppError>;
    async fn count_by_status(&self) -> Result<HashMap<String, u64>, AppError>;
    /// Pending rows whose backoff has elapsed, oldest deadline first.
    async fn due_pending(
        &self,
        now: i64,
        limit: u64,
    ) -> Result<Vec<email_message::Model>, AppError>;

    /// Take ownership of an attempt by incrementing `attempts`.
    ///
    /// `expected_attempts` is the value the caller read, and the update only
    /// applies while it still holds — a compare-and-swap. That is what makes
    /// the claim exclusive: without it, two drains (or a drain and a manual
    /// retry) that both read `attempts = 0` would both match and both send.
    ///
    /// `false` means the row moved on (claimed elsewhere, no longer pending, or
    /// its backoff has not elapsed), so the caller must not try to send. The
    /// counter is bumped by the claim rather than after the send so a process
    /// that dies mid-delivery still counts the attempt.
    async fn claim(&self, id: i32, expected_attempts: i32, now: i64) -> Result<bool, AppError>;
    /// Delivered: clear the ciphertext — a delivered body must not survive in
    /// the database — and record the delivery time.
    async fn mark_sent(&self, id: i32, sent_at: i64) -> Result<(), AppError>;
    /// This attempt failed. `Some(next_attempt_at)` keeps the row pending for a
    /// retry; `None` gives up and marks it failed.
    async fn mark_attempt_failed(
        &self,
        id: i32,
        next_attempt_at: Option<i64>,
        error: &str,
    ) -> Result<(), AppError>;
    /// Put a failed row back in the queue for a manual retry.
    async fn requeue(&self, id: i32, now: i64) -> Result<bool, AppError>;

    async fn delete_by_id(&self, id: i32) -> Result<bool, AppError>;
    /// Delete every finished (sent or failed) row, returning how many.
    async fn delete_finished(&self) -> Result<u64, AppError>;
    /// Retention: drop sent rows older than `sent_before`, failed rows older
    /// than `failed_before`, and the oldest finished rows beyond `max_finished`.
    async fn prune(
        &self,
        sent_before: i64,
        failed_before: i64,
        max_finished: u64,
    ) -> Result<u64, AppError>;
}

pub struct DbEmailMessageRepository {
    db: Arc<DatabaseConnection>,
}

impl DbEmailMessageRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    fn finished_filter() -> sea_orm::sea_query::SimpleExpr {
        email_message::Column::Status.is_in([Status::Sent.id(), Status::Failed.id()])
    }
}

#[async_trait]
impl EmailMessageRepository for DbEmailMessageRepository {
    async fn insert(&self, message: NewEmailMessage) -> Result<email_message::Model, AppError> {
        let model = email_message::ActiveModel {
            id: sea_orm::NotSet,
            kind: Set(message.kind),
            to_address: Set(message.to_address),
            user_id: Set(message.user_id),
            subject: Set(message.subject),
            body_enc: Set(Some(message.body_enc)),
            status: Set(Status::Pending.id().to_string()),
            attempts: Set(0),
            // Due immediately: the caller attempts delivery right after
            // queueing, and a restart has to find it retryable too.
            next_attempt_at: Set(message.created_at),
            last_error: Set(None),
            created_at: Set(message.created_at),
            sent_at: Set(None),
        };
        Ok(model.insert(self.db.as_ref()).await?)
    }

    async fn find_by_id(&self, id: i32) -> Result<Option<email_message::Model>, AppError> {
        Ok(email_message::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?)
    }

    async fn list_recent(
        &self,
        limit: u64,
        status: Option<Status>,
    ) -> Result<Vec<email_message::Model>, AppError> {
        let mut query = email_message::Entity::find();
        if let Some(status) = status {
            query = query.filter(email_message::Column::Status.eq(status.id()));
        }
        Ok(query
            .order_by_desc(email_message::Column::Id)
            .limit(limit)
            .all(self.db.as_ref())
            .await?)
    }

    async fn list_recent_page(
        &self,
        status: Option<Status>,
        limit: u64,
        offset: u64,
    ) -> Result<Vec<email_message::Model>, AppError> {
        let mut query = email_message::Entity::find();
        if let Some(status) = status {
            query = query.filter(email_message::Column::Status.eq(status.id()));
        }
        Ok(query
            .order_by_desc(email_message::Column::Id)
            .limit(limit)
            .offset(offset)
            .all(self.db.as_ref())
            .await?)
    }

    async fn count_by_status(&self) -> Result<HashMap<String, u64>, AppError> {
        // Three counts instead of a GROUP BY: the status set is closed and
        // small, and this keeps the result independent of how the database
        // reports aggregate rows.
        let mut counts = HashMap::new();
        for status in [Status::Pending, Status::Sent, Status::Failed] {
            let count = email_message::Entity::find()
                .filter(email_message::Column::Status.eq(status.id()))
                .count(self.db.as_ref())
                .await?;
            counts.insert(status.id().to_string(), count);
        }
        Ok(counts)
    }

    async fn due_pending(
        &self,
        now: i64,
        limit: u64,
    ) -> Result<Vec<email_message::Model>, AppError> {
        Ok(email_message::Entity::find()
            .filter(email_message::Column::Status.eq(Status::Pending.id()))
            .filter(email_message::Column::NextAttemptAt.lte(now))
            .order_by_asc(email_message::Column::NextAttemptAt)
            .order_by_asc(email_message::Column::Id)
            .limit(limit)
            .all(self.db.as_ref())
            .await?)
    }

    async fn claim(&self, id: i32, expected_attempts: i32, now: i64) -> Result<bool, AppError> {
        let result = email_message::Entity::update_many()
            .filter(email_message::Column::Id.eq(id))
            .filter(email_message::Column::Status.eq(Status::Pending.id()))
            .filter(email_message::Column::Attempts.eq(expected_attempts))
            .filter(email_message::Column::NextAttemptAt.lte(now))
            .col_expr(
                email_message::Column::Attempts,
                sea_orm::sea_query::Expr::col(email_message::Column::Attempts).add(1),
            )
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected == 1)
    }

    async fn mark_sent(&self, id: i32, sent_at: i64) -> Result<(), AppError> {
        email_message::Entity::update_many()
            .filter(email_message::Column::Id.eq(id))
            .set(email_message::ActiveModel {
                status: Set(Status::Sent.id().to_string()),
                sent_at: Set(Some(sent_at)),
                last_error: Set(None),
                // The rendered body — which for a password reset contains the
                // one-time link — must not outlive delivery.
                body_enc: Set(None),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn mark_attempt_failed(
        &self,
        id: i32,
        next_attempt_at: Option<i64>,
        error: &str,
    ) -> Result<(), AppError> {
        let (status, next) = match next_attempt_at {
            Some(next) => (Status::Pending, next),
            // Kept as `failed` rather than deleted: the admin page shows the
            // reason, and the encrypted body stays until retention prunes it so
            // a manual retry can still deliver.
            None => (Status::Failed, 0),
        };
        email_message::Entity::update_many()
            .filter(email_message::Column::Id.eq(id))
            .set(email_message::ActiveModel {
                status: Set(status.id().to_string()),
                next_attempt_at: Set(next),
                last_error: Set(Some(error.to_string())),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn requeue(&self, id: i32, now: i64) -> Result<bool, AppError> {
        let result = email_message::Entity::update_many()
            .filter(email_message::Column::Id.eq(id))
            .filter(email_message::Column::Status.eq(Status::Failed.id()))
            .set(email_message::ActiveModel {
                status: Set(Status::Pending.id().to_string()),
                next_attempt_at: Set(now),
                last_error: Set(None),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected == 1)
    }

    async fn delete_by_id(&self, id: i32) -> Result<bool, AppError> {
        let result = email_message::Entity::delete_many()
            .filter(email_message::Column::Id.eq(id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected == 1)
    }

    async fn delete_finished(&self) -> Result<u64, AppError> {
        let result = email_message::Entity::delete_many()
            .filter(Self::finished_filter())
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn prune(
        &self,
        sent_before: i64,
        failed_before: i64,
        max_finished: u64,
    ) -> Result<u64, AppError> {
        let mut deleted: u64 = 0;

        for (status, cutoff) in [(Status::Sent, sent_before), (Status::Failed, failed_before)] {
            deleted += email_message::Entity::delete_many()
                .filter(email_message::Column::Status.eq(status.id()))
                .filter(email_message::Column::CreatedAt.lt(cutoff))
                .exec(self.db.as_ref())
                .await?
                .rows_affected;
        }

        // Cap the table as a whole, oldest first, so a mail loop cannot grow it
        // without bound however recent the rows are.
        let finished = email_message::Entity::find()
            .filter(Self::finished_filter())
            .order_by_desc(email_message::Column::Id)
            .all(self.db.as_ref())
            .await?;
        if finished.len() as u64 > max_finished {
            let victims: Vec<i32> = finished
                .into_iter()
                .skip(max_finished as usize)
                .map(|row| row.id)
                .collect();
            deleted += email_message::Entity::delete_many()
                .filter(email_message::Column::Id.is_in(victims))
                .exec(self.db.as_ref())
                .await?
                .rows_affected;
        }

        Ok(deleted)
    }
}
