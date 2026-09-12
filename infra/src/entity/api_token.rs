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
    /// Where this token came from, as a session-source id: `web`,
    /// `web_client_login`, `client` or `client_sso`.
    ///
    /// Recorded explicitly rather than inferred from `platform`, which is only
    /// set when the client reported device details and is therefore also absent
    /// for a browser session, for a client that sent no device info, and for
    /// the desktop client's "view on website" handoff.
    #[sea_orm(not_null, default_value = "web")]
    pub source: String,
    /// The `User-Agent` of the login that created the token, for the sources
    /// that have one. Browser sessions carry no `device_name`, so this is the
    /// only thing that identifies them.
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
