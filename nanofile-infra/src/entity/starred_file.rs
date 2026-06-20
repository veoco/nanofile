use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "starred_files")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(not_null, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null)]
    pub path: String,
    #[sea_orm(not_null)]
    pub user_id: i32,
    #[sea_orm(not_null)]
    pub is_dir: bool,
    #[sea_orm(not_null)]
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
