use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "fs_objects")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(not_null, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null, length = 40)]
    pub fs_id: String,
    #[sea_orm(not_null)]
    pub obj_type: i8,
    #[sea_orm(column_type = "Text", not_null)]
    pub data: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::repo::Entity",
        from = "Column::RepoId",
        to = "super::repo::Column::Id"
    )]
    Repo,
}

impl Related<super::repo::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Repo.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
