use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A one-time SSO / "view on website" login token. `token` is stored as a SHA-256 hash;
/// `api_token` holds the plaintext API token minted on completion, which `poll_sso_link`
/// must re-present (a documented limitation).
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
    /// When the browser first opened `/client-sso/{token}/`; the 300s soft timeout starts here.
    pub accessed_at: Option<i64>,
    /// Desktop client version (`shib_client_version`) for device-bound token creation.
    pub client_version: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
