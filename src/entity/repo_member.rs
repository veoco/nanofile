use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "repo_members")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(not_null, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null)]
    pub user_id: i32,
    #[sea_orm(not_null, default_value = "rw")]
    pub permission: String,
    #[sea_orm(not_null)]
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::repo::Entity",
        from = "Column::RepoId",
        to = "super::repo::Column::Id"
    )]
    Repo,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id"
    )]
    User,
}

impl Related<super::repo::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Repo.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
