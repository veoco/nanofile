use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A unified API key.
///
/// One row carries any combination of capabilities, optionally narrowed to libraries
/// with a read/write ceiling. The secret is never stored: `key_hash` is
/// `hex(sha256(raw))`, so a leaked database yields no usable credential and the
/// plaintext is shown once. Capabilities are the canonical sorted comma list.
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
    /// First characters of the plaintext, so the owner can tell two keys apart. `NULL`
    /// for rows migrated from the legacy WebDAV key table (hashes cannot be reversed).
    pub key_prefix: Option<String>,
    #[sea_orm(not_null)]
    pub capabilities: String,
    /// When true the key applies to every library its owner can access and
    /// `api_key_repos` must be empty; when false at least one binding is required.
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
