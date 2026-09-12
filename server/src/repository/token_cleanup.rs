//! Periodic cleanup of expired bearer/one-time tokens.
//!
//! Several token tables are only ever validated lazily, so nothing removed
//! rows once they expired. Anonymous endpoints in particular mint
//! `sso_login_tokens` (and the web login flow mints `client_login_tokens`),
//! which an unauthenticated caller could grow without bound. This module
//! deletes expired rows hourly from the scheduler.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use base::error::AppError;
use infra::entity::{
    api_key, api_token, client_login_token, password_reset_token, s2fa_token, sso_login_token,
    sync_token,
};

/// Client-login tokens are single-use and short-lived; anything older than an
/// hour is certainly unusable (they carry no expiry column).
const CLIENT_LOGIN_STALE_SECS: i64 = 3600;

/// Delete expired/used token rows and return the number of rows removed.
pub async fn delete_expired_tokens(db: &DatabaseConnection, now: i64) -> Result<u64, AppError> {
    let mut total: u64 = 0;

    total += sync_token::Entity::delete_many()
        .filter(sync_token::Column::ExpiresAt.is_not_null())
        .filter(sync_token::Column::ExpiresAt.lt(now))
        .exec(db)
        .await?
        .rows_affected;

    total += api_token::Entity::delete_many()
        .filter(api_token::Column::ExpiresAt.is_not_null())
        .filter(api_token::Column::ExpiresAt.lt(now))
        .exec(db)
        .await?
        .rows_affected;

    // Unified API keys may also expire. A cached lookup checks `expires_at`
    // before use, so a row that outlives its expiry by up to one cleanup cycle
    // is still rejected.
    total += api_key::Entity::delete_many()
        .filter(api_key::Column::ExpiresAt.is_not_null())
        .filter(api_key::Column::ExpiresAt.lt(now))
        .exec(db)
        .await?
        .rows_affected;

    total += s2fa_token::Entity::delete_many()
        .filter(s2fa_token::Column::ExpiresAt.lt(now))
        .exec(db)
        .await?
        .rows_affected;

    total += sso_login_token::Entity::delete_many()
        .filter(sso_login_token::Column::ExpiresAt.is_not_null())
        .filter(sso_login_token::Column::ExpiresAt.lt(now))
        .exec(db)
        .await?
        .rows_affected;

    total += client_login_token::Entity::delete_many()
        .filter(client_login_token::Column::CreatedAt.lt(now - CLIENT_LOGIN_STALE_SECS))
        .exec(db)
        .await?
        .rows_affected;

    // Password-reset tokens are single-use and short-lived.
    total += password_reset_token::Entity::delete_many()
        .filter(password_reset_token::Column::Used.eq(true))
        .exec(db)
        .await?
        .rows_affected;
    total += password_reset_token::Entity::delete_many()
        .filter(password_reset_token::Column::ExpiresAt.lt(now))
        .exec(db)
        .await?
        .rows_affected;

    Ok(total)
}
