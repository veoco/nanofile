//! Registration service for UI layer refactoring.
//!
//! This service handles user registration with invitation code validation.

use std::sync::Arc;

use crate::repository::Repositories;
use crate::service::auth::password::{hash_password, validate_password};
use base::error::AppError;
use infra::entity::user;

/// Parameters for user registration.
pub struct RegistrationParams {
    pub email: String,
    pub password: String,
    pub invitation_code: String,
    pub password_min_length: usize,
    pub require_strong_password: bool,
    pub password_hash_iterations: u32,
}

/// Result of successful registration.
pub struct RegistrationResult {
    pub user: user::Model,
}

/// Service handling user registration with invitation codes.
pub struct RegistrationService {
    repos: Arc<Repositories>,
}

impl RegistrationService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// Register a new user with the given parameters.
    ///
    /// This method:
    /// 1. Validates the invitation code
    /// 2. Validates email format and uniqueness
    /// 3. Validates password strength
    /// 4. Creates the user
    /// 5. Marks the invitation code as used
    pub async fn register(
        &self,
        params: RegistrationParams,
    ) -> Result<RegistrationResult, AppError> {
        // 1. Validate invitation code
        let code_record = self
            .repos
            .invitation_code
            .find_by_code(&params.invitation_code)
            .await?
            .ok_or_else(|| AppError::BadRequest("Invalid invitation code.".to_string()))?;

        if code_record.used_by.is_some() {
            return Err(AppError::BadRequest(
                "This invitation code has already been used.".to_string(),
            ));
        }

        // 2. Check email binding
        if let Some(ref bound_email) = code_record.email
            && bound_email.to_lowercase() != params.email.to_lowercase()
        {
            return Err(AppError::BadRequest(
                "This invitation code is bound to a different email address.".to_string(),
            ));
        }

        // 3. Validate email format
        if !params.email.contains('@') || params.email.len() > 254 {
            return Err(AppError::BadRequest("Invalid email address.".to_string()));
        }

        // 4. Validate email uniqueness
        let existing = self.repos.user.find_by_email(&params.email).await?;
        if existing.is_some() {
            return Err(AppError::BadRequest(
                "A user with this email already exists.".to_string(),
            ));
        }

        // 5. Validate password strength
        validate_password(
            &params.password,
            params.password_min_length as u32,
            params.require_strong_password,
        )
        .map_err(AppError::BadRequest)?;

        // 6. Create the user
        let password_hash = hash_password(&params.password, params.password_hash_iterations);
        let new_user = self
            .repos
            .user
            .create_with_inviter(params.email, password_hash, Some(code_record.creator_id))
            .await?;

        // 7. Mark invitation code as used
        let now = chrono::Utc::now().timestamp();
        self.repos
            .invitation_code
            .mark_as_used(code_record.id, new_user.id, now)
            .await?;

        Ok(RegistrationResult { user: new_user })
    }
}
