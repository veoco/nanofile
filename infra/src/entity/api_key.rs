use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A unified API key.
///
/// One row can carry any combination of capabilities (the catalog lives in
/// `server::domain::capability`), optionally narrowed to a set of libraries with
/// a per-library read/write ceiling. The secret itself is
/// never stored: `key_hash` is `hex(sha256(raw))`, so a leaked database does not
/// yield a usable credential and the plaintext is shown to its creator exactly
/// once.
///
/// `capabilities` is the canonical, sorted, comma-separated identifier list
/// produced by `CapabilitySet::to_canonical`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "api_keys")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(not_null)]
    pub user_id: i32,
    #[sea_orm(not_null)]
    pub name: String,
    #[sea_orm(unique, not_null, length = 64)]
    pub key_hash: String,
    /// First characters of the plaintext, kept so the owner can tell two keys
    /// apart in the UI. `NULL` for rows migrated from the legacy per-library
    /// WebDAV key table, whose hashes cannot be reversed into a prefix.
    pub key_prefix: Option<String>,
    #[sea_orm(not_null)]
    pub capabilities: String,
    /// When true the key applies to every library its owner can access, and
    /// `api_key_repos` must be empty for it. When false at least one binding is
    /// required.
    #[sea_orm(not_null, default_value = false)]
    pub all_repos: bool,
    #[sea_orm(not_null)]
    pub created_at: i64,
    /// `NULL` means the key never expires.
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id"
    )]
    User,
    #[sea_orm(has_many = "super::api_key_repo::Entity")]
    Repos,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl Related<super::api_key_repo::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Repos.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
