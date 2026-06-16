use rand::Rng;
use sha1::{Digest, Sha1};

const TOKEN_LEN: usize = 40;

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
    let mut token = [0u8; 12];
    rand::rng().fill_bytes(&mut token);
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, token)
}

pub fn generate_upload_link_token() -> String {
    let mut token = [0u8; 12];
    rand::rng().fill_bytes(&mut token);
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, token)
}

pub fn generate_backup_code() -> String {
    let mut code = [0u8; 4];
    rand::rng().fill_bytes(&mut code);
    hex::encode(code).to_uppercase()
}

pub fn hash_backup_code(code: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(code.as_bytes());
    hex::encode(hasher.finalize())
}
