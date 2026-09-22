//! Persistence for the admin-managed settings (`settings` table).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};

use base::error::AppError;
use infra::entity::settings;
use infra::settings::SettingRow;

/// Read and write the settings rows as a whole map.
///
/// The whole table is a few dozen rows and every reader needs all of them (the
/// layering is decided per key), so there is no per-key read path to maintain.
#[async_trait]
pub trait SettingsRepository: Send + Sync {
    /// Every stored value, keyed the way the catalog names it.
    async fn load_all(&self) -> Result<BTreeMap<String, SettingRow>, AppError>;
    /// Insert or replace the given values, stamping each with `now`/`updated_by`.
    async fn upsert_many(
        &self,
        values: &BTreeMap<String, String>,
        updated_by: Option<i32>,
        now: i64,
    ) -> Result<(), AppError>;
    /// Drop the rows for `keys`, so those settings fall back to the config file
    /// (or the built-in default) again.
    async fn delete_keys(&self, keys: &[String]) -> Result<usize, AppError>;
}

pub struct DbSettingsRepository {
    db: Arc<DatabaseConnection>,
}

impl DbSettingsRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SettingsRepository for DbSettingsRepository {
    async fn load_all(&self) -> Result<BTreeMap<String, SettingRow>, AppError> {
        let rows = settings::Entity::find()
            .order_by_asc(settings::Column::Key)
            .all(self.db.as_ref())
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.key,
                    SettingRow {
                        value: row.value,
                        updated_at: row.updated_at,
                        updated_by: row.updated_by,
                    },
                )
            })
            .collect())
    }

    async fn upsert_many(
        &self,
        values: &BTreeMap<String, String>,
        updated_by: Option<i32>,
        now: i64,
    ) -> Result<(), AppError> {
        if values.is_empty() {
            return Ok(());
        }
        let models: Vec<settings::ActiveModel> = values
            .iter()
            .map(|(key, value)| settings::ActiveModel {
                key: Set(key.clone()),
                value: Set(value.clone()),
                updated_at: Set(now),
                updated_by: Set(updated_by),
            })
            .collect();

        // One statement for the batch, and one transaction around it: a save
        // that touches several keys must not leave half of them applied.
        let txn = self.db.begin().await?;
        settings::Entity::insert_many(models)
            .on_conflict(
                OnConflict::column(settings::Column::Key)
                    .update_columns([
                        settings::Column::Value,
                        settings::Column::UpdatedAt,
                        settings::Column::UpdatedBy,
                    ])
                    .to_owned(),
            )
            .exec(&txn)
            .await?;
        txn.commit().await?;
        Ok(())
    }

    async fn delete_keys(&self, keys: &[String]) -> Result<usize, AppError> {
        if keys.is_empty() {
            return Ok(0);
        }
        let result = settings::Entity::delete_many()
            .filter(settings::Column::Key.is_in(keys.to_vec()))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected as usize)
    }
}
