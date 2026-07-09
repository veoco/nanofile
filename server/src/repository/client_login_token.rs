use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait, ModelTrait};
use std::sync::Arc;

use crate::entity::client_login_token;
use crate::error::AppError;

#[async_trait]
pub trait ClientLoginTokenRepository: Send + Sync {
    async fn insert(&self, model: client_login_token::ActiveModel) -> Result<(), AppError>;
    async fn find_by_token(
        &self,
        token: &str,
    ) -> Result<Option<client_login_token::Model>, AppError>;
    async fn delete(&self, model: client_login_token::Model) -> Result<(), AppError>;
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
}
