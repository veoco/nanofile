use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::gc_state;

/// Key for the time of the last full audit pass, as Unix epoch seconds.
///
/// Kept as the pre-per-library row: an installation upgrading from a build that
/// recorded one timestamp for the whole server still gets it as the baseline, so the
/// first incremental pass does not audit every library again.
pub const LAST_AUDIT_AT: &str = "last_audit_at";

/// Key of one library's last completed full audit, as Unix epoch seconds.
pub fn audit_key(repo_id: &str) -> String {
    format!("{LAST_AUDIT_AT}:{repo_id}")
}

/// Small key/value rows for garbage-collection state that has no better home.
#[async_trait]
pub trait GcStateRepository: Send + Sync {
    async fn get_i64(&self, key: &str) -> Result<Option<i64>, AppError>;
    async fn set_i64(&self, key: &str, value: i64) -> Result<(), AppError>;
    /// Forget one key, for a library that no longer exists.
    async fn delete(&self, key: &str) -> Result<(), AppError>;
}

pub struct DbGcStateRepository {
    db: Arc<DatabaseConnection>,
}

impl DbGcStateRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl GcStateRepository for DbGcStateRepository {
    async fn get_i64(&self, key: &str) -> Result<Option<i64>, AppError> {
        let row = gc_state::Entity::find()
            .filter(gc_state::Column::Key.eq(key))
            .one(self.db.as_ref())
            .await?;
        Ok(row.and_then(|row| row.value.parse::<i64>().ok()))
    }

    async fn set_i64(&self, key: &str, value: i64) -> Result<(), AppError> {
        gc_state::Entity::insert(gc_state::ActiveModel {
            key: Set(key.to_string()),
            value: Set(value.to_string()),
            updated_at: Set(chrono::Utc::now().timestamp()),
        })
        .on_conflict(
            sea_orm::sea_query::OnConflict::column(gc_state::Column::Key)
                .update_columns([gc_state::Column::Value, gc_state::Column::UpdatedAt])
                .to_owned(),
        )
        .exec(self.db.as_ref())
        .await?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), AppError> {
        gc_state::Entity::delete_many()
            .filter(gc_state::Column::Key.eq(key))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
