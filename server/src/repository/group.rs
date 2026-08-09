use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::group;

#[async_trait]
pub trait GroupRepository: Send + Sync {
    async fn find_by_id(&self, id: i32) -> Result<Option<group::Model>, AppError>;
    /// Fetch multiple groups by id in one query (chunked for SQLite).
    async fn find_by_ids(&self, group_ids: &[i32]) -> Result<Vec<group::Model>, AppError>;
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

    async fn find_by_ids(&self, group_ids: &[i32]) -> Result<Vec<group::Model>, AppError> {
        if group_ids.is_empty() {
            return Ok(Vec::new());
        }
        const IN_BATCH: usize = 500;
        let mut out = Vec::new();
        for chunk in group_ids.chunks(IN_BATCH) {
            let rows = group::Entity::find()
                .filter(group::Column::Id.is_in(chunk.to_vec()))
                .all(self.db.as_ref())
                .await?;
            out.extend(rows);
        }
        Ok(out)
    }
}
