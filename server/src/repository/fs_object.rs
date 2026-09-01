use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect};
use std::collections::HashSet;
use std::sync::Arc;

use base::error::AppError;
use infra::entity::fs_object;

#[async_trait]
pub trait FsObjectRepository: Send + Sync {
    async fn find_by_repo_and_fs_id(
        &self,
        repo_id: &str,
        fs_id: &str,
    ) -> Result<Option<fs_object::Model>, AppError>;
    async fn exists_by_repo_and_fs_id(&self, repo_id: &str, fs_id: &str) -> Result<bool, AppError>;
    async fn find_by_repo_and_fs_ids(
        &self,
        repo_id: &str,
        fs_ids: &[String],
    ) -> Result<Vec<fs_object::Model>, AppError>;
    /// Project only `fs_id` for the given ids of a repo, without loading the
    /// potentially large `data` JSON column. Returns the set of ids that exist.
    async fn find_existing_fs_ids(
        &self,
        repo_id: &str,
        fs_ids: &[String],
    ) -> Result<HashSet<String>, AppError>;
    async fn insert_many(&self, models: Vec<fs_object::ActiveModel>) -> Result<(), AppError>;
    /// Get all fs objects of a single repo (used by garbage collection).
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<fs_object::Model>, AppError>;
    /// Project only `(id, fs_id)` for every object of a repo, without loading
    /// the potentially large `data` JSON column (used by garbage collection).
    /// fs_id is returned as its raw 20-byte SHA-1 to avoid per-element heap
    /// allocation in the caller's set.
    async fn find_ids_and_fs_ids_by_repo_id(
        &self,
        repo_id: &str,
    ) -> Result<Vec<(i64, [u8; 20])>, AppError>;
    async fn delete_many_by_ids(&self, ids: Vec<i64>) -> Result<(), AppError>;
}

pub struct DbFsObjectRepository {
    db: Arc<DatabaseConnection>,
}

impl DbFsObjectRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl FsObjectRepository for DbFsObjectRepository {
    async fn find_by_repo_and_fs_id(
        &self,
        repo_id: &str,
        fs_id: &str,
    ) -> Result<Option<fs_object::Model>, AppError> {
        Ok(fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(fs_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn exists_by_repo_and_fs_id(&self, repo_id: &str, fs_id: &str) -> Result<bool, AppError> {
        // Project only the primary key — the `data` column can be a large JSON
        // blob that is irrelevant when all we need is existence.
        let row: Option<(i64,)> = fs_object::Entity::find()
            .select_only()
            .column(fs_object::Column::Id)
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(fs_id))
            .into_tuple()
            .one(self.db.as_ref())
            .await?;
        Ok(row.is_some())
    }

    async fn find_by_repo_and_fs_ids(
        &self,
        repo_id: &str,
        fs_ids: &[String],
    ) -> Result<Vec<fs_object::Model>, AppError> {
        if fs_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        // Chunk the IN list to stay under SQLite's ~999 variable limit — a wide
        // directory can have thousands of direct children, and a single
        // unchunked IN query would error.
        for chunk in fs_ids.chunks(500) {
            out.extend(
                fs_object::Entity::find()
                    .filter(fs_object::Column::RepoId.eq(repo_id))
                    .filter(fs_object::Column::FsId.is_in(chunk))
                    .all(self.db.as_ref())
                    .await?,
            );
        }
        Ok(out)
    }

    async fn find_existing_fs_ids(
        &self,
        repo_id: &str,
        fs_ids: &[String],
    ) -> Result<HashSet<String>, AppError> {
        let mut out = HashSet::new();
        for chunk in fs_ids.chunks(500) {
            let found: Vec<(String,)> = fs_object::Entity::find()
                .select_only()
                .column(fs_object::Column::FsId)
                .filter(fs_object::Column::RepoId.eq(repo_id))
                .filter(fs_object::Column::FsId.is_in(chunk.to_vec()))
                .into_tuple()
                .all(self.db.as_ref())
                .await?;
            out.extend(found.into_iter().map(|(id,)| id));
        }
        Ok(out)
    }

    async fn insert_many(&self, models: Vec<fs_object::ActiveModel>) -> Result<(), AppError> {
        if models.is_empty() {
            return Ok(());
        }
        fs_object::Entity::insert_many(models)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    fs_object::Column::RepoId,
                    fs_object::Column::FsId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<fs_object::Model>, AppError> {
        Ok(fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_ids_and_fs_ids_by_repo_id(
        &self,
        repo_id: &str,
    ) -> Result<Vec<(i64, [u8; 20])>, AppError> {
        let rows: Vec<(i64, String)> = fs_object::Entity::find()
            .select_only()
            .column(fs_object::Column::Id)
            .column(fs_object::Column::FsId)
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .into_tuple()
            .all(self.db.as_ref())
            .await?;
        Ok(rows
            .into_iter()
            .map(|(id, fs_id)| {
                let mut buf = [0u8; 20];
                let _ = hex::decode_to_slice(&fs_id, &mut buf);
                (id, buf)
            })
            .collect())
    }

    async fn delete_many_by_ids(&self, ids: Vec<i64>) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        const IN_BATCH: usize = 500;
        for chunk in ids.chunks(IN_BATCH) {
            fs_object::Entity::delete_many()
                .filter(fs_object::Column::Id.is_in(chunk.to_vec()))
                .exec(self.db.as_ref())
                .await?;
        }
        Ok(())
    }
}
