use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "api_tokens")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(not_null)]
    pub user_id: i32,
    #[sea_orm(unique, not_null, length = 64)]
    pub token: String,
    #[sea_orm(not_null)]
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub device_id: Option<String>,
    pub platform: Option<String>,
    pub device_name: Option<String>,
    pub client_version: Option<String>,
    #[sea_orm(not_null, default_value = false)]
    pub is_pending: bool,
    /// Where the token came from: `web`, `web_client_login`, `client` or `client_sso`.
    /// Recorded explicitly rather than inferred from `platform`, which is absent for
    /// browser sessions, clients without device info, and the "view on website" handoff.
    #[sea_orm(not_null, default_value = "web")]
    pub source: String,
    /// The login's `User-Agent`, where the source has one. Browser sessions carry no
    /// `device_name`, so this is the only identifier.
    pub user_agent: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
