use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait};
use std::sync::Arc;

use crate::entity::client_login_token;
use crate::error::AppError;

#[async_trait]
pub trait ClientLoginTokenRepository: Send + Sync {
    async fn insert(&self, model: client_login_token::ActiveModel) -> Result<(), AppError>;
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
}
