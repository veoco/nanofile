use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A tag definition scoped to a repository (`repo_tags` table).
///
/// Mirrors seafile's `RepoTags` model: tags carry a name and a color and are
/// exposed through the metadata service API as `_id` / `_tag_name` /
/// `_tag_color`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "repo_tags")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(not_null, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null)]
    pub name: String,
    #[sea_orm(not_null)]
    pub color: String,
    #[sea_orm(not_null)]
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::file_tag::Entity")]
    FileTag,
}

impl Related<super::file_tag::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::FileTag.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
