use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::email_settings;

/// Everything the settings form can change.
///
/// Kept separate from the entity so the service never has to build an
/// `ActiveModel` (with its `NotSet`/`Unchanged` subtleties) from form input.
#[derive(Clone, Debug, Default)]
pub struct EmailSettingsUpdate {
    pub paused: bool,
    pub host: String,
    pub port: i32,
    pub tls: String,
    pub username: String,
    /// `None` leaves the stored password alone, `Some(None)` clears it and
    /// `Some(Some(..))` replaces it. Three-way on purpose: a password field
    /// that renders empty on GET cannot be distinguished from "clear it" by
    /// value alone.
    pub password: Option<Option<String>>,
    pub from_address: String,
    pub from_name: String,
    pub timeout_secs: i32,
    pub max_attempts: i32,
    pub notify_new_device: bool,
    pub notify_api_key_created: bool,
    pub notify_new_login: bool,
    pub updated_by: Option<i32>,
}

#[async_trait]
pub trait EmailSettingsRepository: Send + Sync {
    /// The settings row, or `None` when the deployment has never saved one.
    async fn get(&self) -> Result<Option<email_settings::Model>, AppError>;
    /// Create the row when absent, update it otherwise.
    async fn upsert(&self, update: EmailSettingsUpdate, now: i64) -> Result<(), AppError>;
}

pub struct DbEmailSettingsRepository {
    db: Arc<DatabaseConnection>,
}

impl DbEmailSettingsRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

/// The settings table is a single row; every reader and writer agrees on the id.
pub const SETTINGS_ROW_ID: i32 = 1;

#[async_trait]
impl EmailSettingsRepository for DbEmailSettingsRepository {
    async fn get(&self) -> Result<Option<email_settings::Model>, AppError> {
        Ok(email_settings::Entity::find_by_id(SETTINGS_ROW_ID)
            .one(self.db.as_ref())
            .await?)
    }

    async fn upsert(&self, update: EmailSettingsUpdate, now: i64) -> Result<(), AppError> {
        let existing = self.get().await?;

        // Only an explicit password choice touches the ciphertext; every other
        // field is written from the form as submitted.
        let password_enc = match (update.password, &existing) {
            (Some(value), _) => value,
            (None, Some(row)) => row.password_enc.clone(),
            (None, None) => None,
        };

        let model = email_settings::ActiveModel {
            id: Set(SETTINGS_ROW_ID),
            paused: Set(update.paused),
            host: Set(update.host),
            port: Set(update.port),
            tls: Set(update.tls),
            username: Set(update.username),
            password_enc: Set(password_enc),
            from_address: Set(update.from_address),
            from_name: Set(update.from_name),
            timeout_secs: Set(update.timeout_secs),
            max_attempts: Set(update.max_attempts),
            notify_new_device: Set(update.notify_new_device),
            notify_api_key_created: Set(update.notify_api_key_created),
            notify_new_login: Set(update.notify_new_login),
            updated_at: Set(now),
            updated_by: Set(update.updated_by),
        };

        match existing {
            Some(_) => {
                email_settings::Entity::update_many()
                    .set(model)
                    .filter(email_settings::Column::Id.eq(SETTINGS_ROW_ID))
                    .exec(self.db.as_ref())
                    .await?;
            }
            None => {
                email_settings::Entity::insert(model)
                    .exec(self.db.as_ref())
                    .await?;
            }
        }
        Ok(())
    }
}
