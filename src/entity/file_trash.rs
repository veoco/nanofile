use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "file_trash")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(not_null, length = 255)]
    pub user: String,
    #[sea_orm(not_null, length = 128)]
    pub obj_type: String,
    #[sea_orm(not_null, length = 40)]
    pub obj_id: String,
    #[sea_orm(not_null, length = 255)]
    pub obj_name: String,
    #[sea_orm(not_null)]
    pub delete_time: i64,
    #[sea_orm(not_null, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null, length = 40)]
    pub commit_id: String,
    #[sea_orm(not_null)]
    pub path: String,
    #[sea_orm(not_null)]
    pub size: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
