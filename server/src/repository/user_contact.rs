use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::entity::user_contact;
use crate::error::AppError;

#[async_trait]
pub trait UserContactRepository: Send + Sync {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Vec<user_contact::Model>, AppError>;
}

pub struct DbUserContactRepository {
    db: Arc<DatabaseConnection>,
}

impl DbUserContactRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl UserContactRepository for DbUserContactRepository {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Vec<user_contact::Model>, AppError> {
        Ok(user_contact::Entity::find()
            .filter(user_contact::Column::UserId.eq(user_id))
            .all(self.db.as_ref())
            .await?)
    }
}
