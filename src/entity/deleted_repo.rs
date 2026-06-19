use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "deleted_repos")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null, length = 255)]
    pub repo_name: String,
    #[sea_orm(nullable, length = 40)]
    pub head_id: Option<String>,
    #[sea_orm(not_null)]
    pub owner_id: i32,
    #[sea_orm(not_null)]
    pub size: i64,
    #[sea_orm(not_null)]
    pub del_time: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
