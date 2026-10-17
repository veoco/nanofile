//! The first account an installation ever has.
//!
//! Both entry points that create it — the first-run setup page and the
//! `[admin_init]` configuration block — go through [`create_first_admin`], so the
//! password policy and the administrator columns cannot differ between them.
//! `nanofile adduser` deliberately stays out: it is an operator tool with its own
//! output and "already exists" semantics.

use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};

use base::error::AppError;
use infra::config::Config;
use infra::entity::user;

use super::password::{hash_password_async, validate_password};

/// The password policy for an account created before anybody could sign in.
#[derive(Debug, Clone, Copy)]
pub struct AdminPolicy {
    pub password_min_length: u32,
    pub require_strong_password: bool,
    pub password_hash_iterations: u32,
}

impl AdminPolicy {
    pub fn from_config(config: &Config) -> Self {
        Self {
            password_min_length: config.auth.password_min_length,
            require_strong_password: config.auth.require_strong_password,
            password_hash_iterations: config.auth.password_hash_iterations,
        }
    }
}

/// Create the first administrator, applying the configured password policy.
///
/// The plaintext is hashed here and never returned, so the password the operator
/// typed lives only in the request body and this function's stack.
///
/// Returns the new account's id.
pub async fn create_first_admin(
    db: &DatabaseConnection,
    email: &str,
    password: &str,
    policy: AdminPolicy,
) -> Result<i32, AppError> {
    validate_password(
        password,
        policy.password_min_length,
        policy.require_strong_password,
    )
    .map_err(AppError::BadRequest)?;

    let password_hash =
        hash_password_async(password.to_string(), policy.password_hash_iterations).await;
    let now = chrono::Utc::now().timestamp();
    let model = user::ActiveModel {
        id: sea_orm::NotSet,
        email: Set(email.to_string()),
        password_hash: Set(password_hash),
        is_active: Set(true),
        is_admin: Set(true),
        created_at: Set(now),
        last_login_at: Set(None),
        invited_by: Set(None),
        storage_quota: sea_orm::NotSet,
        name: sea_orm::NotSet,
        display_name: sea_orm::NotSet,
        language: sea_orm::NotSet,
    };
    let created = model.insert(db).await?;
    Ok(created.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_comes_from_the_auth_settings() {
        let mut config = Config::default();
        config.auth.password_min_length = 12;
        config.auth.require_strong_password = true;
        config.auth.password_hash_iterations = 1000;

        let policy = AdminPolicy::from_config(&config);
        assert_eq!(policy.password_min_length, 12);
        assert!(policy.require_strong_password);
        assert_eq!(policy.password_hash_iterations, 1000);
    }
}
