use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::entity::wiki;
use crate::error::AppError;

#[async_trait]
pub trait WikiRepository: Send + Sync {
    async fn find_by_id(&self, id: i32) -> Result<Option<wiki::Model>, AppError>;
    async fn find_by_owner_id(&self, owner_id: i32) -> Result<Vec<wiki::Model>, AppError>;
    async fn rename(&self, id: i32, name: &str) -> Result<(), AppError>;
    async fn set_published(&self, id: i32, published: bool) -> Result<(), AppError>;
    async fn delete_by_id(&self, id: i32) -> Result<(), AppError>;
}

pub struct DbWikiRepository {
    db: Arc<DatabaseConnection>,
}

impl DbWikiRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl WikiRepository for DbWikiRepository {
    async fn find_by_id(&self, id: i32) -> Result<Option<wiki::Model>, AppError> {
        Ok(wiki::Entity::find_by_id(id).one(self.db.as_ref()).await?)
    }

    async fn find_by_owner_id(&self, owner_id: i32) -> Result<Vec<wiki::Model>, AppError> {
        Ok(wiki::Entity::find()
            .filter(wiki::Column::OwnerId.eq(owner_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn rename(&self, id: i32, name: &str) -> Result<(), AppError> {
        let w = wiki::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?
            .ok_or_else(|| AppError::NotFound("wiki not found".into()))?;
        let mut active: wiki::ActiveModel = w.into();
        active.name = Set(name.to_string());
        active.update(self.db.as_ref()).await?;
        Ok(())
    }

    async fn set_published(&self, id: i32, published: bool) -> Result<(), AppError> {
        let w = wiki::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?
            .ok_or_else(|| AppError::NotFound("wiki not found".into()))?;
        let mut active: wiki::ActiveModel = w.into();
        active.published = Set(Some(published));
        active.update(self.db.as_ref()).await?;
        Ok(())
    }

    async fn delete_by_id(&self, id: i32) -> Result<(), AppError> {
        wiki::Entity::delete_by_id(id).exec(self.db.as_ref()).await?;
        Ok(())
    }
}
