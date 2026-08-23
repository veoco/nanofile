use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A one-time "view on website" client login token.
///
/// `token` is stored as a SHA-256 hash (bearer tokens are never stored in
/// plaintext).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "client_login_tokens")]
pub struct Model {
    #[sea_orm(primary_key, not_null, length = 64)]
    pub token: String,
    #[sea_orm(not_null, length = 255)]
    pub username: String,
    #[sea_orm(not_null)]
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
