use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "client_login_tokens")]
pub struct Model {
    #[sea_orm(primary_key, not_null, length = 32)]
    pub token: String,
    #[sea_orm(not_null, length = 255)]
    pub username: String,
    #[sea_orm(not_null)]
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
