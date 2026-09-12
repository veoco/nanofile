use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One library binding of an [`api_key`](super::api_key).
///
/// `permission` is a ceiling, not the effective permission: a request is
/// allowed only when the key's capabilities, this ceiling and the caller's
/// library membership all agree. `"r"` downgrades every write capability to a
/// denial for this library.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "api_key_repos")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(not_null)]
    pub key_id: i32,
    #[sea_orm(not_null, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null)]
    pub permission: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::api_key::Entity",
        from = "Column::KeyId",
        to = "super::api_key::Column::Id"
    )]
    ApiKey,
    #[sea_orm(
        belongs_to = "super::repo::Entity",
        from = "Column::RepoId",
        to = "super::repo::Column::Id"
    )]
    Repo,
}

impl Related<super::api_key::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ApiKey.def()
    }
}

impl Related<super::repo::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Repo.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
