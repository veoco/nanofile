use rand::Rng;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "invitation_codes")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique, not_null, length = 32)]
    pub code: String,
    pub email: Option<String>,
    #[sea_orm(not_null)]
    pub creator_id: i32,
    #[sea_orm(not_null)]
    pub created_at: i64,
    pub used_by: Option<i32>,
    pub used_at: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::CreatorId",
        to = "super::user::Column::Id"
    )]
    Creator,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UsedBy",
        to = "super::user::Column::Id"
    )]
    UsedBy,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Creator.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

/// Generate a random 32-character hex invitation code.
pub fn generate_invitation_code() -> String {
    let mut raw = [0u8; 16];
    rand::rng().fill_bytes(&mut raw);
    hex::encode(raw)
}
