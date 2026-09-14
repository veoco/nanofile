use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::api_token;

use crate::domain::session_source::SessionSource;

/// Parameters for creating a session token.
pub struct CreateSessionTokenParams {
    pub user_id: i32,
    pub token: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub device_id: Option<String>,
    pub platform: Option<String>,
    pub device_name: Option<String>,
    pub client_version: Option<String>,
    /// Marks a 2FA pending token (must not be usable as a full session).
    pub is_pending: bool,
    /// Which login produced this token.
    pub source: SessionSource,
    /// The login's `User-Agent`, for the sources that carry one. Browser
    /// sessions have no device name, so this is what identifies them.
    pub user_agent: Option<String>,
}

#[async_trait]
pub trait ApiTokenRepository: Send + Sync {
    async fn find_by_token(&self, token: &str) -> Result<Option<api_token::Model>, AppError>;
    /// Every session a user holds, newest first.
    ///
    /// The inventory needs all of them, not only the ones carrying a
    /// `platform`: a browser session has no device details, and a client that
    /// reports none is still a client. Pending 2FA tokens are left out — they
    /// are a five-minute half-credential, not something the owner holds.
    async fn list_sessions(&self, user_id: i32) -> Result<Vec<api_token::Model>, AppError>;
    /// One of a user's sessions, so a guessed id cannot reach another account's
    /// row.
    async fn find_by_id_and_user(
        &self,
        token_id: i32,
        user_id: i32,
    ) -> Result<Option<api_token::Model>, AppError>;
    /// Revoke one session, scoped to its owner.
    async fn delete_by_id_and_user(&self, token_id: i32, user_id: i32) -> Result<u64, AppError>;
    async fn delete_many_by_device(&self, device_id: &str) -> Result<(), AppError>;
    async fn delete_many_by_user_platform_device(
        &self,
        user_id: i32,
        platform: &str,
        device_id: &str,
    ) -> Result<u64, AppError>;
    async fn delete_many_by_user_and_device(
        &self,
        user_id: i32,
        device_id: &str,
    ) -> Result<u64, AppError>;
    async fn delete_by_token(&self, token: &str) -> Result<(), AppError>;
    async fn insert(&self, model: api_token::ActiveModel) -> Result<(), AppError>;
    async fn delete_many_by_user_id(&self, user_id: i32) -> Result<(), AppError>;
    /// Delete every account token for a user **except** one raw session token
    /// (used when changing a password while keeping the acting session alive).
    async fn delete_many_by_user_id_except(
        &self,
        user_id: i32,
        keep_raw_token: &str,
    ) -> Result<(), AppError>;

    /// Delete every *browser session* a user holds except one row id.
    ///
    /// The bulk "sign out my other sessions" action. Only browser-held tokens
    /// go: a client application's session is a device the owner manages from
    /// the device list, not something a web page quietly signs out.
    ///
    /// A row whose `source` this build does not recognise is treated as a
    /// browser session, matching the inventory's safe default: showing (and
    /// revoking) an unknown credential beats leaving it untouched. The filter
    /// is therefore written as "everything that is not a known client", not as
    /// an allow-list of the two browser sources.
    ///
    /// Returns how many rows were removed.
    async fn delete_browser_sessions_except(
        &self,
        user_id: i32,
        keep_id: Option<i32>,
    ) -> Result<u64, AppError>;

    // ── Methods for UI layer refactoring ───────────────────────────────
    /// Create a session token and return the model.
    async fn create_session_token(
        &self,
        params: CreateSessionTokenParams,
    ) -> Result<api_token::Model, AppError>;
}

pub struct DbApiTokenRepository {
    db: Arc<DatabaseConnection>,
}

impl DbApiTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ApiTokenRepository for DbApiTokenRepository {
    async fn find_by_token(&self, token: &str) -> Result<Option<api_token::Model>, AppError> {
        Ok(api_token::Entity::find()
            .filter(api_token::Column::Token.eq(crate::service::auth::token::hash_token(token)))
            .one(self.db.as_ref())
            .await?)
    }

    async fn list_sessions(&self, user_id: i32) -> Result<Vec<api_token::Model>, AppError> {
        Ok(api_token::Entity::find()
            .filter(api_token::Column::UserId.eq(user_id))
            .filter(api_token::Column::IsPending.eq(false))
            .order_by_desc(api_token::Column::CreatedAt)
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_id_and_user(
        &self,
        token_id: i32,
        user_id: i32,
    ) -> Result<Option<api_token::Model>, AppError> {
        Ok(api_token::Entity::find()
            .filter(api_token::Column::Id.eq(token_id))
            .filter(api_token::Column::UserId.eq(user_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn delete_by_id_and_user(&self, token_id: i32, user_id: i32) -> Result<u64, AppError> {
        let result = api_token::Entity::delete_many()
            .filter(api_token::Column::Id.eq(token_id))
            .filter(api_token::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn delete_many_by_device(&self, device_id: &str) -> Result<(), AppError> {
        api_token::Entity::delete_many()
            .filter(api_token::Column::DeviceId.eq(device_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_token(&self, token: &str) -> Result<(), AppError> {
        api_token::Entity::delete_many()
            .filter(api_token::Column::Token.eq(crate::service::auth::token::hash_token(token)))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_many_by_user_platform_device(
        &self,
        user_id: i32,
        platform: &str,
        device_id: &str,
    ) -> Result<u64, AppError> {
        let result = api_token::Entity::delete_many()
            .filter(api_token::Column::UserId.eq(user_id))
            .filter(api_token::Column::Platform.eq(platform))
            .filter(api_token::Column::DeviceId.eq(device_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn delete_many_by_user_and_device(
        &self,
        user_id: i32,
        device_id: &str,
    ) -> Result<u64, AppError> {
        let result = api_token::Entity::delete_many()
            .filter(api_token::Column::UserId.eq(user_id))
            .filter(api_token::Column::DeviceId.eq(device_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn insert(&self, model: api_token::ActiveModel) -> Result<(), AppError> {
        api_token::Entity::insert(model)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_many_by_user_id(&self, user_id: i32) -> Result<(), AppError> {
        api_token::Entity::delete_many()
            .filter(api_token::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_many_by_user_id_except(
        &self,
        user_id: i32,
        keep_raw_token: &str,
    ) -> Result<(), AppError> {
        // Tokens are stored hashed; exclude the current session by its hash.
        let keep_hash = crate::service::auth::token::hash_token(keep_raw_token);
        api_token::Entity::delete_many()
            .filter(api_token::Column::UserId.eq(user_id))
            .filter(api_token::Column::Token.ne(keep_hash))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_browser_sessions_except(
        &self,
        user_id: i32,
        keep_id: Option<i32>,
    ) -> Result<u64, AppError> {
        let mut query = api_token::Entity::delete_many()
            .filter(api_token::Column::UserId.eq(user_id))
            .filter(api_token::Column::IsPending.eq(false))
            .filter(
                api_token::Column::Source
                    .is_not_in([SessionSource::Client.id(), SessionSource::ClientSso.id()]),
            );
        if let Some(id) = keep_id {
            query = query.filter(api_token::Column::Id.ne(id));
        }
        let result = query.exec(self.db.as_ref()).await?;
        Ok(result.rows_affected)
    }

    async fn create_session_token(
        &self,
        params: CreateSessionTokenParams,
    ) -> Result<api_token::Model, AppError> {
        let model = api_token::ActiveModel {
            id: sea_orm::NotSet,
            user_id: Set(params.user_id),
            token: Set(crate::service::auth::token::hash_token(&params.token)),
            created_at: Set(params.created_at),
            expires_at: Set(params.expires_at),
            device_id: Set(params.device_id),
            platform: Set(params.platform),
            device_name: Set(params.device_name),
            client_version: Set(params.client_version),
            is_pending: Set(params.is_pending),
            source: Set(params.source.id().to_string()),
            user_agent: Set(params.user_agent),
        };
        Ok(model.insert(self.db.as_ref()).await?)
    }
}
