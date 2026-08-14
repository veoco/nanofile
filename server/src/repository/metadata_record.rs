use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::metadata_record;

#[async_trait]
pub trait MetadataRecordRepository: Send + Sync {
    async fn find_by_repo_id(&self, repo_id: &str)
    -> Result<Vec<metadata_record::Model>, AppError>;
    async fn find_by_repo_and_path(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Result<Vec<metadata_record::Model>, AppError>;
    async fn upsert(
        &self,
        repo_id: &str,
        file_path: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), AppError>;
    /// Store multiple (key, value) pairs for a file path.
    async fn upsert_many(
        &self,
        repo_id: &str,
        file_path: &str,
        fields: &[(String, Option<String>)],
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

    async fn find_by_repo_and_path(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Result<Vec<metadata_record::Model>, AppError> {
        Ok(metadata_record::Entity::find()
            .filter(metadata_record::Column::RepoId.eq(repo_id))
            .filter(metadata_record::Column::FilePath.eq(file_path))
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

    async fn upsert_many(
        &self,
        repo_id: &str,
        file_path: &str,
        fields: &[(String, Option<String>)],
    ) -> Result<(), AppError> {
        if fields.is_empty() {
            return Ok(());
        }
        let db = self.db.as_ref();
        let now = chrono::Utc::now().timestamp();

        // One batched DELETE for all keys (chunked for SQLite), then one
        // batched INSERT — instead of a DELETE+INSERT round-trip per field.
        const IN_BATCH: usize = 500;
        let keys: Vec<String> = fields.iter().map(|(k, _)| k.clone()).collect();
        for chunk in keys.chunks(IN_BATCH) {
            metadata_record::Entity::delete_many()
                .filter(metadata_record::Column::RepoId.eq(repo_id))
                .filter(metadata_record::Column::FilePath.eq(file_path))
                .filter(metadata_record::Column::RecordKey.is_in(chunk.to_vec()))
                .exec(db)
                .await?;
        }

        let models: Vec<metadata_record::ActiveModel> = fields
            .iter()
            .map(|(key, value)| metadata_record::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: Set(repo_id.to_string()),
                file_path: Set(file_path.to_string()),
                record_key: Set(key.clone()),
                record_value: Set(value.clone()),
                created_at: Set(now),
                updated_at: Set(now),
            })
            .collect();

        metadata_record::Entity::insert_many(models)
            .exec(db)
            .await?;
        Ok(())
    }
}
