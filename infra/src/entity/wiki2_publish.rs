use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Seafile wiki2 public-publish configuration. One row per published wiki.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "wiki2_publish")]
pub struct Model {
    #[sea_orm(primary_key, length = 36)]
    pub repo_id: String,
    #[sea_orm(unique, not_null, length = 40)]
    pub publish_url: String,
    #[sea_orm(not_null, length = 255)]
    pub username: String,
    #[sea_orm(not_null)]
    pub created_at: i64,
    #[sea_orm(not_null, default_value = 0)]
    pub visit_count: i32,
    #[sea_orm(not_null, default_value = false)]
    pub enable_server_render: bool,
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
