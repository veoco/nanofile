use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, Statement,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::file_tag;
use infra::entity::repo_tag;

/// A tag attached to a file, with the tag's display details joined in.
#[derive(Clone, Debug)]
pub struct TagOnPath {
    pub file_path: String,
    pub tag_id: i32,
    pub tag_name: String,
    pub tag_color: String,
}

#[async_trait]
pub trait FileTagRepository: Send + Sync {
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<file_tag::Model>, AppError>;
    async fn find_by_repo_and_path(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Result<Vec<file_tag::Model>, AppError>;
    async fn find_by_repo_and_tag_id(
        &self,
        repo_id: &str,
        tag_id: i32,
    ) -> Result<Vec<file_tag::Model>, AppError>;
    async fn delete_by_id(&self, id: i32) -> Result<(), AppError>;
    /// Replace the tags of a single file path with the given set.
    async fn set_for_path(
        &self,
        repo_id: &str,
        file_path: &str,
        tag_ids: &[i32],
    ) -> Result<(), AppError>;
    /// Remove all tag rows for a path and everything below it (delete cleanup).
    async fn delete_by_path_prefix(&self, repo_id: &str, path: &str) -> Result<(), AppError>;
    /// After a rename/move, update tag rows whose path starts with the old path.
    async fn update_paths_for_rename(
        &self,
        old_path: &str,
        new_path: &str,
        repo_id: &str,
    ) -> Result<(), AppError>;
    /// Batch fetch tag details for the given paths (used by the file browser).
    async fn find_tag_details_by_paths(
        &self,
        repo_id: &str,
        paths: &[String],
    ) -> Result<Vec<TagOnPath>, AppError>;
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

    async fn find_by_repo_and_path(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Result<Vec<file_tag::Model>, AppError> {
        Ok(file_tag::Entity::find()
            .filter(file_tag::Column::RepoId.eq(repo_id))
            .filter(file_tag::Column::FilePath.eq(file_path))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo_and_tag_id(
        &self,
        repo_id: &str,
        tag_id: i32,
    ) -> Result<Vec<file_tag::Model>, AppError> {
        Ok(file_tag::Entity::find()
            .filter(file_tag::Column::RepoId.eq(repo_id))
            .filter(file_tag::Column::RepoTagId.eq(tag_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn delete_by_id(&self, id: i32) -> Result<(), AppError> {
        file_tag::Entity::delete_many()
            .filter(file_tag::Column::Id.eq(id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn set_for_path(
        &self,
        repo_id: &str,
        file_path: &str,
        tag_ids: &[i32],
    ) -> Result<(), AppError> {
        let db = self.db.as_ref();
        file_tag::Entity::delete_many()
            .filter(file_tag::Column::RepoId.eq(repo_id))
            .filter(file_tag::Column::FilePath.eq(file_path))
            .exec(db)
            .await?;

        // Dedupe ids: callers may pass duplicates, which would otherwise create
        // duplicate rows (file_tags has no unique constraint).
        let unique: std::collections::HashSet<i32> = tag_ids.iter().copied().collect();
        if unique.is_empty() {
            return Ok(());
        }

        let now = chrono::Utc::now().timestamp();
        let models: Vec<file_tag::ActiveModel> = unique
            .into_iter()
            .map(|tag_id| file_tag::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: Set(repo_id.to_string()),
                file_path: Set(file_path.to_string()),
                repo_tag_id: Set(tag_id),
                created_at: Set(now),
            })
            .collect();
        file_tag::Entity::insert_many(models).exec(db).await?;
        Ok(())
    }

    async fn delete_by_path_prefix(&self, repo_id: &str, path: &str) -> Result<(), AppError> {
        self.db
            .as_ref()
            .execute(Statement::from_sql_and_values(
                self.db.as_ref().get_database_backend(),
                "DELETE FROM file_tags WHERE repo_id = $1 \
                 AND (file_path = $2 OR file_path LIKE $2 || '/%')",
                [repo_id.into(), path.into()],
            ))
            .await?;
        Ok(())
    }

    async fn update_paths_for_rename(
        &self,
        old_path: &str,
        new_path: &str,
        repo_id: &str,
    ) -> Result<(), AppError> {
        self.db
            .as_ref()
            .execute(Statement::from_sql_and_values(
                self.db.as_ref().get_database_backend(),
                "UPDATE file_tags SET file_path = $1 || substr(file_path, length($2) + 1) \
                 WHERE repo_id = $3 AND (file_path = $2 OR file_path LIKE $2 || '/%')",
                [new_path.into(), old_path.into(), repo_id.into()],
            ))
            .await?;
        Ok(())
    }

    async fn find_tag_details_by_paths(
        &self,
        repo_id: &str,
        paths: &[String],
    ) -> Result<Vec<TagOnPath>, AppError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        // Chunk the IN list to stay under SQLite's ~999 variable limit.
        const IN_BATCH: usize = 500;
        let mut out = Vec::new();
        for chunk in paths.chunks(IN_BATCH) {
            let rows = file_tag::Entity::find()
                .filter(file_tag::Column::RepoId.eq(repo_id))
                .filter(file_tag::Column::FilePath.is_in(chunk))
                .find_also_related(repo_tag::Entity)
                .all(self.db.as_ref())
                .await?;

            for (ft, tag) in rows {
                if let Some(tag) = tag {
                    out.push(TagOnPath {
                        file_path: ft.file_path,
                        tag_id: tag.id,
                        tag_name: tag.name,
                        tag_color: tag.color,
                    });
                }
            }
        }
        Ok(out)
    }
}
