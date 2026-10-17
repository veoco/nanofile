use std::collections::HashMap;
use std::sync::Arc;

use crate::repository::Repositories;
use base::error::AppError;
use infra::entity::invitation_code;

/// Invitation code info returned by the service; the timestamps go out raw so the page renders them
/// in the viewer's timezone.
pub struct InvitationInfo {
    pub code: String,
    pub bound_email: Option<String>,
    pub created_at_ts: i64,
    pub used_by_email: Option<String>,
    pub used_at_ts: Option<i64>,
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

        // Batch-load the referenced users in one query instead of one per invitation.
        let used_by_ids: Vec<i32> = codes.iter().filter_map(|c| c.used_by).collect();
        let email_by_id: HashMap<i32, String> = self
            .repos
            .user
            .find_by_ids(&used_by_ids)
            .await?
            .into_iter()
            .map(|u| (u.id, u.email))
            .collect();

        let mut invitations = Vec::with_capacity(codes.len());
        for code in codes {
            let used_by_email = code.used_by.and_then(|uid| email_by_id.get(&uid).cloned());

            invitations.push(InvitationInfo {
                id: code.id,
                code: code.code,
                bound_email: code.email,
                created_at_ts: code.created_at,
                used_by_email,
                used_at_ts: code.used_at,
            });
        }

        Ok(invitations)
    }

    pub async fn generate_invitation(
        &self,
        creator_id: i32,
        email: Option<String>,
    ) -> Result<(), AppError> {
        let code_str = invitation_code::generate_invitation_code();
        let now = chrono::Utc::now().timestamp();

        let email = email
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty());

        self.repos
            .invitation_code
            .create(code_str, email, creator_id, now)
            .await
    }

    pub async fn delete_invitation(&self, creator_id: i32, id: i32) -> Result<(), AppError> {
        self.repos
            .invitation_code
            .delete_by_id_and_creator(id, creator_id)
            .await
    }
}
