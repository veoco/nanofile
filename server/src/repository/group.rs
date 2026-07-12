use async_trait::async_trait;
use sea_orm::{DatabaseConnection, EntityTrait};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::group;

#[async_trait]
pub trait GroupRepository: Send + Sync {
    async fn find_by_id(&self, id: i32) -> Result<Option<group::Model>, AppError>;
}

pub struct DbGroupRepository {
    db: Arc<DatabaseConnection>,
}

impl DbGroupRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl GroupRepository for DbGroupRepository {
    async fn find_by_id(&self, id: i32) -> Result<Option<group::Model>, AppError> {
        Ok(group::Entity::find_by_id(id).one(self.db.as_ref()).await?)
    }
}
