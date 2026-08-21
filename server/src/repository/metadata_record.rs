use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, TransactionTrait};
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
        let repo_id = repo_id.to_string();
        let file_path = file_path.to_string();
        let key = key.to_string();
        let value = value.map(|v| v.to_string());

        // DELETE + INSERT in one transaction so a mid-way failure can't leave a
        // partial (missing) record behind.
        db.transaction(|txn| {
            Box::pin(async move {
                metadata_record::Entity::delete_many()
                    .filter(metadata_record::Column::RepoId.eq(repo_id.as_str()))
                    .filter(metadata_record::Column::FilePath.eq(file_path.as_str()))
                    .filter(metadata_record::Column::RecordKey.eq(key.as_str()))
                    .exec(txn)
                    .await?;

                metadata_record::Entity::insert(metadata_record::ActiveModel {
                    id: sea_orm::NotSet,
                    repo_id: Set(repo_id),
                    file_path: Set(file_path),
                    record_key: Set(key),
                    record_value: Set(value),
                    created_at: Set(now),
                    updated_at: Set(now),
                })
                .exec(txn)
                .await?;
                Ok::<(), sea_orm::DbErr>(())
            })
        })
        .await
        .map_err(|e| match e {
            sea_orm::TransactionError::Connection(e) => {
                AppError::internal(format!("metadata upsert transaction: {e}"))
            }
            sea_orm::TransactionError::Transaction(e) => AppError::from(e),
        })?;
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
