use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "repos")]
pub struct Model {
    #[sea_orm(primary_key, length = 36)]
    pub id: String,
    #[sea_orm(not_null)]
    pub name: String,
    #[sea_orm(not_null, default_value = "")]
    pub description: String,
    #[sea_orm(not_null)]
    pub owner_id: i32,
    #[sea_orm(not_null, default_value = 0)]
    pub encrypted: i8,
    #[sea_orm(not_null, default_value = 0)]
    pub enc_version: i8,
    pub magic: Option<String>,
    pub random_key: Option<String>,
    #[sea_orm(not_null, default_value = "")]
    pub salt: String,
    pub head_commit_id: Option<String>,
    #[sea_orm(not_null, default_value = "rw")]
    pub permission: String,
    #[sea_orm(not_null)]
    pub created_at: i64,
    #[sea_orm(not_null)]
    pub updated_at: i64,
    #[sea_orm(not_null, default_value = 0)]
    pub size: i64,
    #[sea_orm(not_null, default_value = 1)]
    pub repo_version: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::OwnerId",
        to = "super::user::Column::Id"
    )]
    Owner,
    #[sea_orm(has_many = "super::repo_member::Entity")]
    Members,
    #[sea_orm(has_many = "super::commit::Entity")]
    Commits,
    #[sea_orm(has_many = "super::fs_object::Entity")]
    FsObjects,
    #[sea_orm(has_many = "super::sync_token::Entity")]
    SyncTokens,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Owner.def()
    }
}

impl Related<super::repo_member::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Members.def()
    }
}

impl Related<super::commit::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Commits.def()
    }
}

impl Related<super::fs_object::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::FsObjects.def()
    }
}

impl Related<super::sync_token::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SyncTokens.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
