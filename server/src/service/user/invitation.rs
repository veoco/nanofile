use std::sync::Arc;

use crate::repository::Repositories;
use base::error::AppError;
use infra::entity::invitation_code;

/// Invitation code info returned by the service.
pub struct InvitationInfo {
    pub code: String,
    pub bound_email: Option<String>,
    pub created_at: i64,
    pub used_by_email: Option<String>,
    pub used_at: Option<i64>,
    pub id: i32,
}

pub struct InvitationService {
    repos: Arc<Repositories>,
}

impl InvitationService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// List invitation codes created by a user (admin only).
    pub async fn list_invitations(&self, creator_id: i32) -> Result<Vec<InvitationInfo>, AppError> {
        let codes = self
            .repos
            .invitation_code
            .find_by_creator_id(creator_id)
            .await?;

        let mut invitations = Vec::with_capacity(codes.len());
        for code in codes {
            let used_by_email = if let Some(uid) = code.used_by {
                self.repos.user.find_by_id(uid).await?.map(|u| u.email)
            } else {
                None
            };

            invitations.push(InvitationInfo {
                id: code.id,
                code: code.code,
                bound_email: code.email,
                created_at: code.created_at,
                used_by_email,
                used_at: code.used_at,
            });
        }

        Ok(invitations)
    }

    /// Generate a new invitation code.
    pub async fn generate_invitation(
        &self,
        creator_id: i32,
        email: Option<String>,
    ) -> Result<(), AppError> {
        let code_str = invitation_code::generate_invitation_code();
        let now = chrono::Utc::now().timestamp();

        // Trim and validate email if provided.
        let email = email
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty());

        self.repos
            .invitation_code
            .create(code_str, email, creator_id, now)
            .await
    }

    /// Delete an invitation code owned by a user.
    pub async fn delete_invitation(&self, creator_id: i32, id: i32) -> Result<(), AppError> {
        self.repos
            .invitation_code
            .delete_by_id_and_creator(id, creator_id)
            .await
    }
}
