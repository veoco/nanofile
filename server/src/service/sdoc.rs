use std::sync::Arc;

use serde::Serialize;

use crate::repository::Repositories;
use base::error::AppError;

/// Response type for SDoc comment operations.
#[derive(Serialize)]
pub struct CommentResponse {
    pub id: i32,
    pub content: String,
    pub resolved: Option<bool>,
    pub created_at: i64,
    pub user_email: String,
}

pub struct SdocService {
    repos: Arc<Repositories>,
}

impl SdocService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    pub async fn list_comments(&self, doc_uuid: &str) -> Result<Vec<CommentResponse>, AppError> {
        let comments = self.repos.sdoc_comment.find_by_doc_uuid(doc_uuid).await?;
        let mut result = Vec::new();
        for c in comments {
            let user = self.repos.user.find_by_id(c.user_id).await?;
            result.push(CommentResponse {
                id: c.id,
                content: c.content,
                resolved: c.resolved,
                created_at: c.created_at,
                user_email: user.map(|u| u.email).unwrap_or_default(),
            });
        }
        Ok(result)
    }

    pub async fn create_comment(
        &self,
        doc_uuid: &str,
        user_id: i32,
        content: &str,
    ) -> Result<CommentResponse, AppError> {
        let inserted = self
            .repos
            .sdoc_comment
            .create(doc_uuid, user_id, content)
            .await?;
        let user = self.repos.user.find_by_id(user_id).await?;
        Ok(CommentResponse {
            id: inserted.id,
            content: inserted.content,
            resolved: inserted.resolved,
            created_at: inserted.created_at,
            user_email: user.map(|u| u.email).unwrap_or_default(),
        })
    }

    pub async fn resolve_comment(&self, comment_id: i32, resolved: bool) -> Result<(), AppError> {
        self.repos
            .sdoc_comment
            .update_resolved(comment_id, resolved)
            .await?;
        Ok(())
    }

    pub async fn delete_comment(&self, comment_id: i32) -> Result<(), AppError> {
        self.repos.sdoc_comment.delete_by_id(comment_id).await?;
        Ok(())
    }
}
