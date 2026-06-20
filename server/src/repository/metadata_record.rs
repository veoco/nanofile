use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::entity::metadata_record;
use crate::error::AppError;

#[async_trait]
pub trait MetadataRecordRepository: Send + Sync {
    async fn find_by_repo_id(&self, repo_id: &str)
    -> Result<Vec<metadata_record::Model>, AppError>;
    async fn upsert(
        &self,
        repo_id: &str,
        file_path: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), AppError>;
}

pub struct DbMetadataRecordRepository {
    db: Arc<DatabaseConnection>,
}

impl DbMetadataRecordRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MetadataRecordRepository for DbMetadataRecordRepository {
    async fn find_by_repo_id(
        &self,
        repo_id: &str,
    ) -> Result<Vec<metadata_record::Model>, AppError> {
        Ok(metadata_record::Entity::find()
            .filter(metadata_record::Column::RepoId.eq(repo_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn upsert(
        &self,
        repo_id: &str,
        file_path: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), AppError> {
        let db = self.db.as_ref();
        let now = chrono::Utc::now().timestamp();

        metadata_record::Entity::delete_many()
            .filter(metadata_record::Column::RepoId.eq(repo_id))
            .filter(metadata_record::Column::FilePath.eq(file_path))
            .filter(metadata_record::Column::RecordKey.eq(key))
            .exec(db)
            .await?;

        metadata_record::Entity::insert(metadata_record::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            file_path: Set(file_path.to_string()),
            record_key: Set(key.to_string()),
            record_value: Set(value.map(|v| v.to_string())),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .exec(db)
        .await?;
        Ok(())
    }
}
