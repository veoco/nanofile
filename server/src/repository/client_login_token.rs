use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait, ModelTrait, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::client_login_token;

pub struct CreateClientLoginTokenParams {
    pub token: String,
    pub username: String,
    pub created_at: i64,
}

#[async_trait]
pub trait ClientLoginTokenRepository: Send + Sync {
    async fn insert(&self, model: client_login_token::ActiveModel) -> Result<(), AppError>;
    async fn find_by_token(
        &self,
        token: &str,
    ) -> Result<Option<client_login_token::Model>, AppError>;
    async fn delete(&self, model: client_login_token::Model) -> Result<(), AppError>;
    async fn create_client_login_token(
        &self,
        params: CreateClientLoginTokenParams,
    ) -> Result<client_login_token::Model, AppError>;
}

pub struct DbClientLoginTokenRepository {
    db: Arc<DatabaseConnection>,
}

impl DbClientLoginTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ClientLoginTokenRepository for DbClientLoginTokenRepository {
    async fn insert(&self, model: client_login_token::ActiveModel) -> Result<(), AppError> {
        client_login_token::Entity::insert(model)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn find_by_token(
        &self,
        token: &str,
    ) -> Result<Option<client_login_token::Model>, AppError> {
        use sea_orm::ColumnTrait;
        use sea_orm::QueryFilter;

        Ok(client_login_token::Entity::find()
            .filter(client_login_token::Column::Token.eq(token))
            .one(self.db.as_ref())
            .await?)
    }

    async fn delete(&self, model: client_login_token::Model) -> Result<(), AppError> {
        model.delete(self.db.as_ref()).await?;
        Ok(())
    }

    async fn create_client_login_token(
        &self,
        params: CreateClientLoginTokenParams,
    ) -> Result<client_login_token::Model, AppError> {
        let token = params.token.clone();
        client_login_token::Entity::insert(client_login_token::ActiveModel {
            token: Set(params.token),
            username: Set(params.username),
            created_at: Set(params.created_at),
        })
        .exec(self.db.as_ref())
        .await?;
        self.find_by_token(&token)
            .await?
            .ok_or_else(|| AppError::Internal("failed to find created client login token".into()))
    }
}
