use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Association between a file path and a repo tag (`file_tags` table).
///
/// The API-facing `file_tag_id` is this table's `id`, and `repo_tag_id`
/// references a row in `repo_tags`. The file is identified by its full path
/// (seafile uses a `FileUUIDMap`; here the path is stored directly).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "file_tags")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(not_null, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null)]
    pub file_path: String,
    #[sea_orm(not_null)]
    pub repo_tag_id: i32,
    #[sea_orm(not_null)]
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::repo_tag::Entity",
        from = "Column::RepoTagId",
        to = "super::repo_tag::Column::Id"
    )]
    RepoTag,
}

impl Related<super::repo_tag::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RepoTag.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
