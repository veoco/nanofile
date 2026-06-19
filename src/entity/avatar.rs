use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "avatars")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique, length = 255)]
    pub email: String,
    #[sea_orm(length = 512)]
    pub avatar_file_name: String,
    #[sea_orm(length = 64)]
    pub mime_type: String,
    pub file_size: i32,
    pub date_uploaded: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
