use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A one-time SSO / "view on website" login token.
///
/// `token` is stored as a SHA-256 hash (bearer tokens are never stored in
/// plaintext). `api_token` holds the plaintext API token minted on completion —
/// the `poll_sso_link` flow must re-present it to the client, so it cannot be
/// hashed (a documented limitation).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sso_login_tokens")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique, not_null, length = 64)]
    pub token: String,
    pub platform: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    #[sea_orm(not_null, default_value = "pending")]
    pub status: String,
    pub username: Option<String>,
    pub api_token: Option<String>,
    #[sea_orm(not_null)]
    pub created_at: i64,
    pub expires_at: Option<i64>,
    /// Unix timestamp when the browser first opened `/client-sso/{token}/`.
    /// The seahub-compatible soft timeout (300s) is measured from here.
    pub accessed_at: Option<i64>,
    /// Desktop client version (from `shib_client_version`) for device-bound
    /// API-token creation on completion.
    pub client_version: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
