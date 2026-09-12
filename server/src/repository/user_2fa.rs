use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::user_2fa;

#[async_trait]
pub trait User2faRepository: Send + Sync {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Option<user_2fa::Model>, AppError>;
    async fn get_or_create(
        &self,
        user_id: i32,
        totp_secret: String,
    ) -> Result<user_2fa::Model, AppError>;
    async fn set_enabled(&self, user_id: i32, enabled: bool, now: i64) -> Result<(), AppError>;
    /// Record the highest TOTP time step accepted for this user (replay guard).
    async fn set_last_used_step(&self, user_id: i32, step: i64) -> Result<(), AppError>;
    /// Atomically claim `step` as used, returning `true` only for the winner.
    ///
    /// Prefer this to [`Self::set_last_used_step`] when the result gates
    /// authentication: it closes the read-then-write replay window.
    async fn consume_step(&self, user_id: i32, step: i64) -> Result<bool, AppError>;
    async fn delete_by_user_id(&self, user_id: i32) -> Result<(), AppError>;
}

pub struct DbUser2faRepository {
    db: Arc<DatabaseConnection>,
    /// Encrypts the TOTP seed at rest. The seed is password-equivalent: anyone
    /// who reads it can mint valid codes, so a leaked database must not yield
    /// usable second factors.
    cipher: Arc<infra::crypto::totp_encryption::TotpCipher>,
}

impl DbUser2faRepository {
    pub fn new(
        db: Arc<DatabaseConnection>,
        cipher: Arc<infra::crypto::totp_encryption::TotpCipher>,
    ) -> Self {
        Self { db, cipher }
    }

    /// Decrypt a stored seed.
    ///
    /// An encrypted value that cannot be decrypted (e.g. the server secret was
    /// rotated) is a hard error: 2FA verification must fail closed rather than
    /// fall back to a seed nobody can vouch for.
    fn decrypt_secret(&self, model: user_2fa::Model) -> Result<user_2fa::Model, AppError> {
        let decrypted = self.cipher.decrypt(&model.totp_secret).ok_or_else(|| {
            AppError::Internal(format!(
                "TOTP seed for user {} could not be decrypted; the server secret may have changed",
                model.user_id
            ))
        })?;
        Ok(user_2fa::Model {
            totp_secret: decrypted,
            ..model
        })
    }
}

#[async_trait]
impl User2faRepository for DbUser2faRepository {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Option<user_2fa::Model>, AppError> {
        match user_2fa::Entity::find_by_id(user_id)
            .one(self.db.as_ref())
            .await?
        {
            Some(model) => Ok(Some(self.decrypt_secret(model)?)),
            None => Ok(None),
        }
    }

    async fn get_or_create(
        &self,
        user_id: i32,
        totp_secret: String,
    ) -> Result<user_2fa::Model, AppError> {
        let existing = user_2fa::Entity::find_by_id(user_id)
            .one(self.db.as_ref())
            .await?;
        if let Some(model) = existing {
            self.decrypt_secret(model)
        } else {
            // Store the seed encrypted; callers keep working with the plaintext
            // base32 value they passed in.
            let stored = self.cipher.encrypt(&totp_secret);
            let model = user_2fa::ActiveModel {
                user_id: Set(user_id),
                totp_secret: Set(stored),
                algorithm: Set("SHA1".to_string()),
                digits: Set(6i16),
                period: Set(30i16),
                enabled: Set(false),
                enabled_at: Set(None),
                last_used_step: Set(None),
            };
            let inserted = model.insert(self.db.as_ref()).await?;
            Ok(user_2fa::Model {
                totp_secret,
                ..inserted
            })
        }
    }

    async fn set_enabled(&self, user_id: i32, enabled: bool, now: i64) -> Result<(), AppError> {
        let result = user_2fa::Entity::update_many()
            .filter(user_2fa::Column::UserId.eq(user_id))
            .set(user_2fa::ActiveModel {
                enabled: Set(enabled),
                enabled_at: Set(if enabled { Some(now) } else { None }),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        if result.rows_affected == 0 {
            return Err(AppError::BadRequest("2FA not set up".into()));
        }
        Ok(())
    }

    async fn set_last_used_step(&self, user_id: i32, step: i64) -> Result<(), AppError> {
        user_2fa::Entity::update_many()
            .filter(user_2fa::Column::UserId.eq(user_id))
            .set(user_2fa::ActiveModel {
                last_used_step: Set(Some(step)),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    /// Claim a TOTP time step, returning whether this call won it.
    ///
    /// The `last_used_step < step` predicate makes the claim atomic: two
    /// concurrent submissions of the same observed code both reach this
    /// statement, but only one updates a row, so only one authenticates. A
    /// read-then-write guard (what this used to be) had a window in which both
    /// callers saw the old step and both were accepted.
    async fn consume_step(&self, user_id: i32, step: i64) -> Result<bool, AppError> {
        let result = user_2fa::Entity::update_many()
            .filter(user_2fa::Column::UserId.eq(user_id))
            .filter(
                sea_orm::Condition::any()
                    .add(user_2fa::Column::LastUsedStep.is_null())
                    .add(user_2fa::Column::LastUsedStep.lt(step)),
            )
            .set(user_2fa::ActiveModel {
                last_used_step: Set(Some(step)),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected == 1)
    }

    async fn delete_by_user_id(&self, user_id: i32) -> Result<(), AppError> {
        user_2fa::Entity::delete_many()
            .filter(user_2fa::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
