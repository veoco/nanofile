use rand::Rng;

use base::error::AppError;

const TOKEN_LEN: usize = 40;

/// Return the existing sync token for a repo/user, or create a new one.
///
/// Requires read permission on the repo (a sync token grants repo access).
pub async fn ensure_sync_token(
    repos: &crate::repository::Repositories,
    repo_id: &str,
    user_id: i32,
) -> Result<String, AppError> {
    crate::domain::permission::check_repo_read_permission(repos.member.as_ref(), repo_id, user_id)
        .await?;

    if let Some(existing) = repos
        .sync_token
        .find_by_repo_and_user(repo_id, user_id)
        .await?
    {
        return Ok(existing.token);
    }

    let token_value = generate_sync_token();
    let now = chrono::Utc::now().timestamp();
    repos
        .sync_token
        .create(repo_id, user_id, token_value.clone(), None, now)
        .await?;
    Ok(token_value)
}

pub fn generate_api_token() -> String {
    let mut token = [0u8; TOKEN_LEN / 2];
    rand::rng().fill_bytes(&mut token);
    hex::encode(token)
}

pub fn generate_sync_token() -> String {
    let mut token = [0u8; TOKEN_LEN / 2];
    rand::rng().fill_bytes(&mut token);
    hex::encode(token)
}

pub fn generate_share_link_token() -> String {
    let mut token = [0u8; 16];
    rand::rng().fill_bytes(&mut token);
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, token)
}

pub fn generate_upload_link_token() -> String {
    let mut token = [0u8; 16];
    rand::rng().fill_bytes(&mut token);
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, token)
}

pub fn generate_backup_code() -> String {
    let mut code = [0u8; 4];
    rand::rng().fill_bytes(&mut code);
    hex::encode(code).to_uppercase()
}
