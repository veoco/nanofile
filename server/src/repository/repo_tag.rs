use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::repo_tag;

/// A tag definition within a repo.
pub struct TagInput {
    pub name: String,
    pub color: String,
}

#[async_trait]
pub trait RepoTagRepository: Send + Sync {
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<repo_tag::Model>, AppError>;
    /// Paginated variant of `find_by_repo_id` with the same stable ordering
    /// (created_at, id), pushing limit/offset down to SQL.
    async fn find_by_repo_id_paginated(
        &self,
        repo_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<repo_tag::Model>, AppError>;
    async fn find_by_id(&self, id: i32) -> Result<Option<repo_tag::Model>, AppError>;
    /// Fetch multiple tags by id in one query (chunked for SQLite).
    async fn find_by_ids(&self, ids: &[i32]) -> Result<Vec<repo_tag::Model>, AppError>;
    async fn find_by_repo_and_name(
        &self,
        repo_id: &str,
        name: &str,
    ) -> Result<Option<repo_tag::Model>, AppError>;
    /// Fetch tags by name in one batched query (chunked for SQLite).
    async fn find_by_names(
        &self,
        repo_id: &str,
        names: &[String],
    ) -> Result<Vec<repo_tag::Model>, AppError>;
    /// Create a tag, returning the created row.
    async fn create(
        &self,
        repo_id: &str,
        name: &str,
        color: &str,
    ) -> Result<repo_tag::Model, AppError>;
    /// Bulk create tags, skipping names that already exist in the repo.
    async fn create_many(
        &self,
        repo_id: &str,
        items: &[TagInput],
    ) -> Result<Vec<repo_tag::Model>, AppError>;
    async fn update(&self, id: i32, name: &str, color: &str) -> Result<(), AppError>;
    /// Delete a tag (file_tags referencing it are removed via FK cascade).
    async fn delete_by_id(&self, id: i32) -> Result<(), AppError>;
    /// Delete several tags in one batched query (chunked for SQLite).
    async fn delete_by_ids(&self, ids: &[i32]) -> Result<(), AppError>;
}

pub struct DbRepoTagRepository {
    db: Arc<DatabaseConnection>,
}

impl DbRepoTagRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RepoTagRepository for DbRepoTagRepository {
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<repo_tag::Model>, AppError> {
        Ok(repo_tag::Entity::find()
            .filter(repo_tag::Column::RepoId.eq(repo_id))
            .order_by_asc(repo_tag::Column::CreatedAt)
            .order_by_asc(repo_tag::Column::Id)
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo_id_paginated(
        &self,
        repo_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<repo_tag::Model>, AppError> {
        Ok(repo_tag::Entity::find()
            .filter(repo_tag::Column::RepoId.eq(repo_id))
            .order_by_asc(repo_tag::Column::CreatedAt)
            .order_by_asc(repo_tag::Column::Id)
            .limit(limit as u64)
            .offset(offset as u64)
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_id(&self, id: i32) -> Result<Option<repo_tag::Model>, AppError> {
        Ok(repo_tag::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_ids(&self, ids: &[i32]) -> Result<Vec<repo_tag::Model>, AppError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        const IN_BATCH: usize = 500;
        let mut out = Vec::new();
        for chunk in ids.chunks(IN_BATCH) {
            let rows = repo_tag::Entity::find()
                .filter(repo_tag::Column::Id.is_in(chunk.to_vec()))
                .all(self.db.as_ref())
                .await?;
            out.extend(rows);
        }
        Ok(out)
    }

    async fn find_by_repo_and_name(
        &self,
        repo_id: &str,
        name: &str,
    ) -> Result<Option<repo_tag::Model>, AppError> {
        Ok(repo_tag::Entity::find()
            .filter(repo_tag::Column::RepoId.eq(repo_id))
            .filter(repo_tag::Column::Name.eq(name))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_names(
        &self,
        repo_id: &str,
        names: &[String],
    ) -> Result<Vec<repo_tag::Model>, AppError> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        const IN_BATCH: usize = 500;
        let mut out = Vec::new();
        for chunk in names.chunks(IN_BATCH) {
            let rows = repo_tag::Entity::find()
                .filter(repo_tag::Column::RepoId.eq(repo_id))
                .filter(repo_tag::Column::Name.is_in(chunk.to_vec()))
                .all(self.db.as_ref())
                .await?;
            out.extend(rows);
        }
        Ok(out)
    }

    async fn create(
        &self,
        repo_id: &str,
        name: &str,
        color: &str,
    ) -> Result<repo_tag::Model, AppError> {
        Ok(repo_tag::Entity::insert(repo_tag::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            name: Set(name.to_string()),
            color: Set(color.to_string()),
            created_at: Set(chrono::Utc::now().timestamp()),
        })
        .exec_with_returning(self.db.as_ref())
        .await?)
    }

    async fn create_many(
        &self,
        repo_id: &str,
        items: &[TagInput],
    ) -> Result<Vec<repo_tag::Model>, AppError> {
        let db = self.db.as_ref();
        let now = chrono::Utc::now().timestamp();

        // Deduplicate names (first-seen color wins) so a single name is looked
        // up and inserted only once — a duplicate would collide with the
        // unique (repo_id, name) index.
        let mut name_to_color: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for item in items {
            name_to_color
                .entry(item.name.clone())
                .or_insert_with(|| item.color.clone());
        }
        let names: Vec<String> = name_to_color.keys().cloned().collect();

        // One batched lookup for existing rows.
        let existing = self.find_by_names(repo_id, &names).await?;
        let existing_names: std::collections::HashSet<&str> =
            existing.iter().map(|t| t.name.as_str()).collect();

        // One batched insert for the missing names. SQLite has no multi-row
        // RETURNING, so we re-query all names below instead of reading back the
        // inserted rows.
        let to_insert: Vec<repo_tag::ActiveModel> = name_to_color
            .iter()
            .filter(|(name, _)| !existing_names.contains(name.as_str()))
            .map(|(name, color)| repo_tag::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: Set(repo_id.to_string()),
                name: Set(name.clone()),
                color: Set(color.clone()),
                created_at: Set(now),
            })
            .collect();
        if !to_insert.is_empty() {
            repo_tag::Entity::insert_many(to_insert).exec(db).await?;
        }

        // Re-query all names to get consistent rows (existing + inserted).
        let all = self.find_by_names(repo_id, &names).await?;
        let by_name: std::collections::HashMap<String, repo_tag::Model> =
            all.into_iter().map(|t| (t.name.clone(), t)).collect();

        // Reassemble in input order, preserving the "reuse existing" semantics.
        let mut out: Vec<repo_tag::Model> = Vec::with_capacity(items.len());
        for item in items {
            if let Some(tag) = by_name.get(&item.name) {
                out.push(tag.clone());
            } else {
                // Rare race: the tag disappeared between lookup and re-query.
                let tag = self.create(repo_id, &item.name, &item.color).await?;
                out.push(tag);
            }
        }
        Ok(out)
    }

    async fn update(&self, id: i32, name: &str, color: &str) -> Result<(), AppError> {
        repo_tag::Entity::update_many()
            .filter(repo_tag::Column::Id.eq(id))
            .set(repo_tag::ActiveModel {
                name: Set(name.to_string()),
                color: Set(color.to_string()),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_id(&self, id: i32) -> Result<(), AppError> {
        repo_tag::Entity::delete_many()
            .filter(repo_tag::Column::Id.eq(id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_ids(&self, ids: &[i32]) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        const IN_BATCH: usize = 500;
        for chunk in ids.chunks(IN_BATCH) {
            repo_tag::Entity::delete_many()
                .filter(repo_tag::Column::Id.is_in(chunk.to_vec()))
                .exec(self.db.as_ref())
                .await?;
        }
        Ok(())
    }
}
