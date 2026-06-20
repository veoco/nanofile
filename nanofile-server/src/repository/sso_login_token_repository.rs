use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::entity::sso_login_token;
use crate::error::AppError;

#[async_trait]
pub trait SsoLoginTokenRepository: Send + Sync {
    async fn find_by_token(&self, token: &str) -> Result<Option<sso_login_token::Model>, AppError>;
    async fn insert(&self, model: sso_login_token::ActiveModel) -> Result<(), AppError>;
}

pub struct DbSsoLoginTokenRepository {
    db: Arc<DatabaseConnection>,
}

impl DbSsoLoginTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SsoLoginTokenRepository for DbSsoLoginTokenRepository {
    async fn find_by_token(&self, token: &str) -> Result<Option<sso_login_token::Model>, AppError> {
        Ok(sso_login_token::Entity::find()
            .filter(sso_login_token::Column::Token.eq(token))
            .one(self.db.as_ref())
            .await?)
    }

    async fn insert(&self, model: sso_login_token::ActiveModel) -> Result<(), AppError> {
        sso_login_token::Entity::insert(model)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
