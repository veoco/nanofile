use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::metadata_config;

#[async_trait]
pub trait MetadataConfigRepository: Send + Sync {
    async fn find_by_repo_id(
        &self,
        repo_id: &str,
    ) -> Result<Option<metadata_config::Model>, AppError>;
    async fn upsert(&self, repo_id: &str, enabled: bool) -> Result<(), AppError>;
}

pub struct DbMetadataConfigRepository {
    db: Arc<DatabaseConnection>,
}

impl DbMetadataConfigRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MetadataConfigRepository for DbMetadataConfigRepository {
    async fn find_by_repo_id(
        &self,
        repo_id: &str,
    ) -> Result<Option<metadata_config::Model>, AppError> {
        Ok(metadata_config::Entity::find()
            .filter(metadata_config::Column::RepoId.eq(repo_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn upsert(&self, repo_id: &str, enabled: bool) -> Result<(), AppError> {
        let db = self.db.as_ref();
        let now = chrono::Utc::now().timestamp();

        let existing = metadata_config::Entity::find()
            .filter(metadata_config::Column::RepoId.eq(repo_id))
            .one(db)
            .await?;

        match existing {
            Some(_c) => {
                metadata_config::Entity::update_many()
                    .filter(metadata_config::Column::RepoId.eq(repo_id))
                    .set(metadata_config::ActiveModel {
                        enabled: Set(Some(enabled)),
                        ..Default::default()
                    })
                    .exec(db)
                    .await?;
            }
            None => {
                metadata_config::Entity::insert(metadata_config::ActiveModel {
                    id: sea_orm::NotSet,
                    repo_id: Set(repo_id.to_string()),
                    enabled: Set(Some(enabled)),
                    created_at: Set(now),
                })
                .exec(db)
                .await?;
            }
        }
        Ok(())
    }
}
