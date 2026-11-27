//! Transactional writes for administrator-managed library sharing.
//!
//! `repos.visibility` and the `repo_members` grant rows must move together: a
//! visibility switch that clears members, or a last-member removal that reverts
//! the library to `private`, would otherwise leave a window where the list and
//! the permission check disagree about who can see the library. These three
//! operations are the only writers that need that guarantee, so they live in one
//! narrow repository rather than being spread across the repo and member ports.

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, Set, TransactionTrait,
};

use base::error::AppError;
use infra::entity::{repo, repo_member};

use crate::domain::repo_visibility::RepoVisibility;

#[async_trait]
pub trait LibraryShareRepository: Send + Sync {
    /// Set the library's visibility. `clear_members` deletes every member grant
    /// except the owner's row in the same transaction (`private` and `public` do;
    /// `shared` keeps them).
    async fn set_visibility(
        &self,
        repo_id: &str,
        visibility: RepoVisibility,
        clear_members: bool,
    ) -> Result<(), AppError>;

    /// Insert a member grant, or replace the permission of an existing one. A
    /// `private` library becomes `shared` in the same transaction.
    async fn upsert_member(
        &self,
        repo_id: &str,
        user_id: i32,
        permission: &str,
    ) -> Result<(), AppError>;

    /// Delete a member grant. When the library was `shared` and no other member
    /// remains, it reverts to `private` in the same transaction.
    async fn remove_member(
        &self,
        repo_id: &str,
        owner_id: i32,
        user_id: i32,
    ) -> Result<(), AppError>;
}

pub struct DbLibraryShareRepository {
    db: Arc<DatabaseConnection>,
}

impl DbLibraryShareRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

async fn set_visibility_in<C: ConnectionTrait>(
    conn: &C,
    repo_id: &str,
    visibility: RepoVisibility,
    clear_members: bool,
    owner_id: i32,
) -> Result<(), DbErr> {
    repo::Entity::update_many()
        .filter(repo::Column::Id.eq(repo_id))
        .set(repo::ActiveModel {
            visibility: Set(visibility.as_str().to_string()),
            ..Default::default()
        })
        .exec(conn)
        .await?;

    if clear_members {
        repo_member::Entity::delete_many()
            .filter(repo_member::Column::RepoId.eq(repo_id))
            .filter(repo_member::Column::UserId.ne(owner_id))
            .exec(conn)
            .await?;
    }
    Ok(())
}

#[async_trait]
impl LibraryShareRepository for DbLibraryShareRepository {
    async fn set_visibility(
        &self,
        repo_id: &str,
        visibility: RepoVisibility,
        clear_members: bool,
    ) -> Result<(), AppError> {
        let Some(owner_id) = repo::Entity::find_by_id(repo_id)
            .one(self.db.as_ref())
            .await?
            .map(|r| r.owner_id)
        else {
            return Err(AppError::NotFound("repo not found".into()));
        };

        let txn = self.db.begin().await?;
        set_visibility_in(&txn, repo_id, visibility, clear_members, owner_id).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn upsert_member(
        &self,
        repo_id: &str,
        user_id: i32,
        permission: &str,
    ) -> Result<(), AppError> {
        let txn = self.db.begin().await?;
        let existing = repo_member::Entity::find()
            .filter(repo_member::Column::RepoId.eq(repo_id))
            .filter(repo_member::Column::UserId.eq(user_id))
            .one(&txn)
            .await?;

        if existing.is_some() {
            repo_member::Entity::update_many()
                .filter(repo_member::Column::RepoId.eq(repo_id))
                .filter(repo_member::Column::UserId.eq(user_id))
                .set(repo_member::ActiveModel {
                    permission: Set(permission.to_string()),
                    ..Default::default()
                })
                .exec(&txn)
                .await?;
        } else {
            repo_member::Entity::insert(repo_member::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: Set(repo_id.to_string()),
                user_id: Set(user_id),
                permission: Set(permission.to_string()),
                created_at: Set(chrono::Utc::now().timestamp()),
            })
            .exec(&txn)
            .await?;
        }

        // First grant on a private library makes it explicitly shared; a public
        // library never reaches here because the service rejects that case.
        repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .filter(repo::Column::Visibility.eq(RepoVisibility::Private.as_str()))
            .set(repo::ActiveModel {
                visibility: Set(RepoVisibility::Shared.as_str().to_string()),
                ..Default::default()
            })
            .exec(&txn)
            .await?;

        txn.commit().await?;
        Ok(())
    }

    async fn remove_member(
        &self,
        repo_id: &str,
        owner_id: i32,
        user_id: i32,
    ) -> Result<(), AppError> {
        let txn = self.db.begin().await?;
        repo_member::Entity::delete_many()
            .filter(repo_member::Column::RepoId.eq(repo_id))
            .filter(repo_member::Column::UserId.eq(user_id))
            .exec(&txn)
            .await?;

        let remaining: u64 = repo_member::Entity::find()
            .filter(repo_member::Column::RepoId.eq(repo_id))
            .filter(repo_member::Column::UserId.ne(owner_id))
            .count(&txn)
            .await?;
        if remaining == 0 {
            repo::Entity::update_many()
                .filter(repo::Column::Id.eq(repo_id))
                .filter(repo::Column::Visibility.eq(RepoVisibility::Shared.as_str()))
                .set(repo::ActiveModel {
                    visibility: Set(RepoVisibility::Private.as_str().to_string()),
                    ..Default::default()
                })
                .exec(&txn)
                .await?;
        }
        txn.commit().await?;
        Ok(())
    }
}
