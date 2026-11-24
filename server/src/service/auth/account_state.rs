//! Account usability checks shared by every credential surface.
//!
//! A token, capability URL or WebDAV login is only as good as the account behind
//! it: a deactivated or unverified account must stop working everywhere. The
//! checks had drifted across five call sites, so one path could accept a
//! deactivated account while another rejected it.

use crate::repository::Repositories;
use base::error::AppError;
use infra::entity::user;

/// The account behind a credential.
#[derive(Debug)]
pub enum AccountState {
    /// The account may act.
    Active(user::Model),
    /// The account exists but is deactivated or has not verified its address.
    Inactive,
    /// No such account.
    Missing,
}

impl AccountState {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active(_))
    }
}

/// Load an account and classify whether it may act.
///
/// `email_verified` is part of the "can act" rule on purpose: the token and
/// capability surfaces have always required it, and it is stricter than
/// upstream's token authentication (which only checks `is_active`); this module
/// keeps nanofile's stricter policy in one place instead of relaxing it.
pub async fn load_account_state(
    repos: &Repositories,
    user_id: i32,
) -> Result<AccountState, AppError> {
    match repos.user.find_by_id(user_id).await? {
        None => Ok(AccountState::Missing),
        Some(user) if user_is_active(&user) => Ok(AccountState::Active(user)),
        Some(_) => Ok(AccountState::Inactive),
    }
}

/// Whether an already-loaded account may act.
pub fn user_is_active(user: &user::Model) -> bool {
    user.is_active && user.email_verified
}

/// Convenience for callers that only need the yes/no answer.
pub async fn is_active_account(repos: &Repositories, user_id: i32) -> Result<bool, AppError> {
    Ok(load_account_state(repos, user_id).await?.is_active())
}
