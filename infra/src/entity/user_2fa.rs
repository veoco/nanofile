use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_2fa")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub user_id: i32,
    #[sea_orm(not_null)]
    pub totp_secret: String,
    #[sea_orm(not_null, default_value = "SHA1")]
    pub algorithm: String,
    #[sea_orm(not_null, default_value = 6)]
    pub digits: i16,
    #[sea_orm(not_null, default_value = 30)]
    pub period: i16,
    #[sea_orm(not_null, default_value = false)]
    pub enabled: bool,
    pub enabled_at: Option<i64>,
    /// Highest TOTP time step already accepted for this user.
    ///
    /// Verification allows the previous/current/next step (±30s clock skew), so
    /// without this a code observed by an attacker stays usable for up to ~90
    /// seconds. Rejecting any step `<= last_used_step` makes each code
    /// effectively single-use while keeping the skew tolerance for honest
    /// clients.
    pub last_used_step: Option<i64>,
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
