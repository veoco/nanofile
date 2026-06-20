use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
};
use std::sync::Arc;

use crate::entity::file_tag;
use crate::error::AppError;

#[async_trait]
pub trait FileTagRepository: Send + Sync {
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<file_tag::Model>, AppError>;
    async fn update_file_tags(
        &self,
        repo_id: &str,
        file_path: &str,
        tags: Option<&[serde_json::Value]>,
    ) -> Result<(), AppError>;
}

pub struct DbFileTagRepository {
    db: Arc<DatabaseConnection>,
}

impl DbFileTagRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl FileTagRepository for DbFileTagRepository {
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<file_tag::Model>, AppError> {
        Ok(file_tag::Entity::find()
            .filter(file_tag::Column::RepoId.eq(repo_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn update_file_tags(
        &self,
        repo_id: &str,
        file_path: &str,
        tags: Option<&[serde_json::Value]>,
    ) -> Result<(), AppError> {
        let db = self.db.as_ref();

        if !file_path.is_empty() {
            file_tag::Entity::delete_many()
                .filter(file_tag::Column::RepoId.eq(repo_id))
                .filter(file_tag::Column::FilePath.eq(file_path))
                .exec(db)
                .await?;

            if let Some(tags) = tags {
                let now = chrono::Utc::now().timestamp();
                for tag in tags {
                    if let Some(tag_name) = tag.as_str() {
                        file_tag::Entity::insert(file_tag::ActiveModel {
                            id: sea_orm::NotSet,
                            repo_id: Set(repo_id.to_string()),
                            file_path: Set(file_path.to_string()),
                            tag_name: Set(tag_name.to_string()),
                            created_at: Set(now),
                        })
                        .exec(db)
                        .await?;
                    }
                }
            }
        }
        Ok(())
    }
}
