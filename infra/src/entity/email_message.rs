use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One queued (or already attempted) outbound message.
///
/// This table is both the retry queue and the audit log the admin page reads.
/// The rendered body is stored **encrypted** while it is pending and cleared
/// once the message has been delivered: a password-reset link must be readable
/// only by the recipient, so it can never sit in plaintext in the database.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "email_messages")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// `password_reset` | `new_device` | `api_key_created` | `new_login` | `test`.
    #[sea_orm(not_null)]
    pub kind: String,
    #[sea_orm(not_null)]
    pub to_address: String,
    /// Recipient account, when the message belongs to one. No foreign key: the
    /// log outlives the account it describes.
    #[sea_orm(nullable)]
    pub user_id: Option<i32>,
    #[sea_orm(not_null)]
    pub subject: String,
    /// Ciphertext of the rendered body. `None` once delivered, or after a
    /// deliberate purge of a message that can no longer be retried.
    #[sea_orm(nullable)]
    pub body_enc: Option<String>,
    /// `pending` | `sent` | `failed`.
    #[sea_orm(not_null, default_value = "pending")]
    pub status: String,
    /// Times delivery was attempted. Incremented as part of *claiming* a row,
    /// so a process that dies mid-send leaves the row retryable.
    #[sea_orm(not_null, default_value = 0)]
    pub attempts: i32,
    #[sea_orm(not_null, default_value = 0)]
    pub next_attempt_at: i64,
    #[sea_orm(nullable)]
    pub last_error: Option<String>,
    #[sea_orm(not_null, default_value = 0)]
    pub created_at: i64,
    #[sea_orm(nullable)]
    pub sent_at: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// Delivery state of a queued message, as persisted in [`Model::status`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Sent,
    Failed,
}

impl Status {
    pub const fn id(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Sent => "sent",
            Status::Failed => "failed",
        }
    }

    /// Parse a persisted status. An unrecognised value is `None` rather than a
    /// silent default: the admin page must not display a state the server does
    /// not actually have.
    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Status::Pending),
            "sent" => Some(Status::Sent),
            "failed" => Some(Status::Failed),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_ids_round_trip() {
        for status in [Status::Pending, Status::Sent, Status::Failed] {
            assert_eq!(Status::from_id(status.id()), Some(status));
        }
        assert_eq!(Status::from_id("queued"), None);
    }
}
