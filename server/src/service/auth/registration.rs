//! User registration with invitation-code validation.
//!
//! Invitation registration is trusted and activates the account immediately.
//! Open registration (no invitation code required) creates an unverified account
//! instead: the UI issues an email-verification link, and the login paths refuse
//! the account until that link is used.

use std::sync::Arc;

use crate::repository::Repositories;
use crate::service::auth::password::{hash_password, validate_password};
use base::error::AppError;
use infra::entity::user;

pub struct RegistrationParams {
    pub email: String,
    pub password: String,
    pub invitation_code: Option<String>,
    pub require_invitation: bool,
    pub password_min_length: usize,
    pub require_strong_password: bool,
    pub password_hash_iterations: u32,
}

pub struct RegistrationResult {
    pub user: user::Model,
}

pub struct RegistrationService {
    repos: Arc<Repositories>,
}

impl RegistrationService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// Register a new user with the given parameters.
    pub async fn register(
        &self,
        params: RegistrationParams,
    ) -> Result<RegistrationResult, AppError> {
        let invitation = if params.require_invitation {
            let code = params
                .invitation_code
                .as_deref()
                .map(str::trim)
                .filter(|code| !code.is_empty())
                .ok_or_else(|| {
                    AppError::BadRequest("An invitation code is required.".to_string())
                })?;
            let code_record = self
                .repos
                .invitation_code
                .find_by_code(code)
                .await?
                .ok_or_else(|| AppError::BadRequest("Invalid invitation code.".to_string()))?;

            if code_record.used_by.is_some() {
                return Err(AppError::BadRequest(
                    "This invitation code has already been used.".to_string(),
                ));
            }

            if let Some(ref bound_email) = code_record.email
                && bound_email.to_lowercase() != params.email.to_lowercase()
            {
                return Err(AppError::BadRequest(
                    "This invitation code is bound to a different email address.".to_string(),
                ));
            }

            Some(code_record)
        } else {
            None
        };

        if params.require_invitation {
            if !params.email.contains('@') || params.email.len() > 254 {
                return Err(AppError::BadRequest("Invalid email address.".to_string()));
            }
        } else if !crate::service::mail::settings::is_valid_address(&params.email) {
            // The verification mail is the only way to activate an open
            // registration, so the address has to be one the mailer accepts.
            return Err(AppError::BadRequest("Invalid email address.".to_string()));
        }

        // The answer is deliberately the same as for a bad invitation code in
        // invitation mode: the login and password-reset paths return generic
        // responses so an unauthenticated caller cannot test which addresses are
        // registered. Open registration uses its own generic message for the same
        // reason.
        let existing = self.repos.user.find_by_email(&params.email).await?;
        if existing.is_some() {
            return Err(duplicate_email_error(params.require_invitation));
        }

        validate_password(
            &params.password,
            params.password_min_length as u32,
            params.require_strong_password,
        )
        .map_err(AppError::BadRequest)?;

        let password_hash = hash_password(&params.password, params.password_hash_iterations);
        let new_user = match self
            .repos
            .user
            .create_self_registered(
                params.email.clone(),
                password_hash,
                invitation.as_ref().map(|code| code.creator_id),
                params.require_invitation,
            )
            .await
        {
            Ok(user) => user,
            Err(err) => {
                // A concurrent registration with the same address can win between
                // the read above and this insert. Report the same answer as the
                // pre-check instead of a 500; anything else is a real failure.
                if self
                    .repos
                    .user
                    .find_by_email(&params.email)
                    .await?
                    .is_some()
                {
                    return Err(duplicate_email_error(params.require_invitation));
                }
                return Err(err);
            }
        };

        if let Some(code_record) = invitation {
            // `mark_as_used` is a conditional UPDATE (`used_by IS NULL`), so exactly
            // one concurrent request can win it. Everything before this point is a read
            // plus a 600k-iteration PBKDF2, a wide window: without a real claim,
            // several requests holding the same code each created an account. The
            // loser now deletes the account it just made, so one code yields one account.
            let now = chrono::Utc::now().timestamp();
            if let Err(e) = self
                .repos
                .invitation_code
                .mark_as_used(code_record.id, new_user.id, now)
                .await
            {
                if let Err(cleanup) = self.repos.user.delete_user(new_user.id).await {
                    tracing::error!(
                        "invitation race: could not roll back account {} created with code {}: {cleanup}",
                        new_user.email,
                        code_record.id
                    );
                }
                tracing::warn!(
                    "invitation code {} was claimed concurrently; registration rejected: {e}",
                    code_record.id
                );
                return Err(AppError::BadRequest(
                    "This invitation code has already been used.".to_string(),
                ));
            }
        }

        Ok(RegistrationResult { user: new_user })
    }
}

/// The refusal for an address that already has an account, worded per mode.
fn duplicate_email_error(require_invitation: bool) -> AppError {
    if require_invitation {
        AppError::BadRequest("Invalid invitation code.".to_string())
    } else {
        AppError::BadRequest("This email address cannot be used to create an account.".to_string())
    }
}
