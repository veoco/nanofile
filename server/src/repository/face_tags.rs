//! Persistence for automatic people tags.
//!
//! The scan row is the "already looked at" record; the exclusion row is the
//! user's correction. Neither stores a face crop, embedding or identity.

use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    sea_query::OnConflict,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::{face_tag_exclusion, face_tag_scan};

#[derive(Clone, Debug)]
pub struct ScanRecord {
    pub file_path: String,
    pub fs_id: String,
    pub model_version: i32,
    pub status: String,
    pub reason: Option<String>,
    pub face_count: i32,
    pub max_score: Option<f32>,
    pub attempted_at: i64,
}

#[async_trait]
pub trait FaceTagRepository: Send + Sync {
    async fn find_scan(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Result<Option<face_tag_scan::Model>, AppError>;
    async fn upsert_scan(&self, repo_id: &str, record: &ScanRecord) -> Result<(), AppError>;
    async fn delete_by_path_prefix(&self, repo_id: &str, path: &str) -> Result<(), AppError>;
    async fn update_paths_for_rename(
        &self,
        old_path: &str,
        new_path: &str,
        repo_id: &str,
    ) -> Result<(), AppError>;

    async fn list_exclusions(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Result<Vec<String>, AppError>;
    async fn exclude(
        &self,
        repo_id: &str,
        file_path: &str,
        label_key: &str,
    ) -> Result<(), AppError>;
    async fn include(
        &self,
        repo_id: &str,
        file_path: &str,
        label_key: &str,
    ) -> Result<(), AppError>;
    async fn clear_exclusions_for_path(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Result<(), AppError>;
}

pub struct DbFaceTagRepository {
    db: Arc<DatabaseConnection>,
}

impl DbFaceTagRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl FaceTagRepository for DbFaceTagRepository {
    async fn find_scan(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Result<Option<face_tag_scan::Model>, AppError> {
        Ok(face_tag_scan::Entity::find()
            .filter(face_tag_scan::Column::RepoId.eq(repo_id))
            .filter(face_tag_scan::Column::FilePath.eq(file_path))
            .one(self.db.as_ref())
            .await?)
    }

    async fn upsert_scan(&self, repo_id: &str, record: &ScanRecord) -> Result<(), AppError> {
        let db = self.db.as_ref();
        let existing = self.find_scan(repo_id, &record.file_path).await?;
        match existing {
            Some(row) => {
                face_tag_scan::Entity::update_many()
                    .filter(face_tag_scan::Column::Id.eq(row.id))
                    .set(face_tag_scan::ActiveModel {
                        fs_id: Set(record.fs_id.clone()),
                        model_version: Set(record.model_version),
                        status: Set(record.status.clone()),
                        reason: Set(record.reason.clone()),
                        face_count: Set(record.face_count),
                        max_score: Set(record.max_score),
                        attempted_at: Set(record.attempted_at),
                        ..Default::default()
                    })
                    .exec(db)
                    .await?;
            }
            None => {
                face_tag_scan::Entity::insert(face_tag_scan::ActiveModel {
                    id: sea_orm::NotSet,
                    repo_id: Set(repo_id.to_string()),
                    file_path: Set(record.file_path.clone()),
                    fs_id: Set(record.fs_id.clone()),
                    model_version: Set(record.model_version),
                    status: Set(record.status.clone()),
                    reason: Set(record.reason.clone()),
                    face_count: Set(record.face_count),
                    max_score: Set(record.max_score),
                    attempted_at: Set(record.attempted_at),
                })
                .exec(db)
                .await?;
            }
        }
        Ok(())
    }

    async fn delete_by_path_prefix(&self, repo_id: &str, path: &str) -> Result<(), AppError> {
        let prefix = format!("{path}/");
        let prefix_len = prefix.chars().count();
        for table in ["face_tag_scans", "face_tag_exclusions"] {
            let statement = format!(
                "DELETE FROM {table} WHERE repo_id = '{repo}' \
                 AND (file_path = '{path}' OR substr(file_path, 1, {prefix_len}) = '{prefix}')",
                repo = sql_escape(repo_id),
                path = sql_escape(path),
                prefix = sql_escape(&prefix),
            );
            self.db
                .execute_unprepared(&statement)
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?;
        }
        Ok(())
    }

    async fn update_paths_for_rename(
        &self,
        old_path: &str,
        new_path: &str,
        repo_id: &str,
    ) -> Result<(), AppError> {
        // Two cases: the path itself, and every descendant. `substr` is used
        // instead of LIKE so `%`/`_` in a file name cannot widen the match.
        let old_prefix = format!("{old_path}/");
        let new_prefix = format!("{new_path}/");
        let prefix_len = old_prefix.chars().count();
        let start = prefix_len + 1;
        for table in ["face_tag_scans", "face_tag_exclusions"] {
            let statement = format!(
                "UPDATE {table} SET file_path = CASE \
                   WHEN file_path = '{old}' THEN '{new}' \
                   WHEN substr(file_path, 1, {prefix_len}) = '{oldp}' \
                     THEN '{newp}' || substr(file_path, {start}) \
                   ELSE file_path END \
                 WHERE repo_id = '{repo}' \
                   AND (file_path = '{old}' OR substr(file_path, 1, {prefix_len}) = '{oldp}')",
                old = sql_escape(old_path),
                new = sql_escape(new_path),
                oldp = sql_escape(&old_prefix),
                newp = sql_escape(&new_prefix),
                repo = sql_escape(repo_id),
            );
            self.db
                .execute_unprepared(&statement)
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?;
        }
        Ok(())
    }

    async fn list_exclusions(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Result<Vec<String>, AppError> {
        Ok(face_tag_exclusion::Entity::find()
            .filter(face_tag_exclusion::Column::RepoId.eq(repo_id))
            .filter(face_tag_exclusion::Column::FilePath.eq(file_path))
            .order_by_asc(face_tag_exclusion::Column::LabelKey)
            .all(self.db.as_ref())
            .await?
            .into_iter()
            .map(|row| row.label_key)
            .collect())
    }

    async fn exclude(
        &self,
        repo_id: &str,
        file_path: &str,
        label_key: &str,
    ) -> Result<(), AppError> {
        let existing = face_tag_exclusion::Entity::find()
            .filter(face_tag_exclusion::Column::RepoId.eq(repo_id))
            .filter(face_tag_exclusion::Column::FilePath.eq(file_path))
            .filter(face_tag_exclusion::Column::LabelKey.eq(label_key))
            .one(self.db.as_ref())
            .await?;
        if existing.is_none() {
            face_tag_exclusion::Entity::insert(face_tag_exclusion::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: Set(repo_id.to_string()),
                file_path: Set(file_path.to_string()),
                label_key: Set(label_key.to_string()),
                created_at: Set(chrono::Utc::now().timestamp()),
            })
            .on_conflict(
                OnConflict::columns([
                    face_tag_exclusion::Column::RepoId,
                    face_tag_exclusion::Column::FilePath,
                    face_tag_exclusion::Column::LabelKey,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec(self.db.as_ref())
            .await?;
        }
        Ok(())
    }

    async fn include(
        &self,
        repo_id: &str,
        file_path: &str,
        label_key: &str,
    ) -> Result<(), AppError> {
        face_tag_exclusion::Entity::delete_many()
            .filter(face_tag_exclusion::Column::RepoId.eq(repo_id))
            .filter(face_tag_exclusion::Column::FilePath.eq(file_path))
            .filter(face_tag_exclusion::Column::LabelKey.eq(label_key))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn clear_exclusions_for_path(
        &self,
        repo_id: &str,
        file_path: &str,
    ) -> Result<(), AppError> {
        face_tag_exclusion::Entity::delete_many()
            .filter(face_tag_exclusion::Column::RepoId.eq(repo_id))
            .filter(face_tag_exclusion::Column::FilePath.eq(file_path))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}

fn sql_escape(value: &str) -> String {
    value.replace('\'', "''")
}
