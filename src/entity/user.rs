use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique, not_null)]
    pub email: String,
    #[sea_orm(not_null)]
    pub password_hash: String,
    #[sea_orm(not_null)]
    pub is_active: bool,
    #[sea_orm(not_null)]
    pub is_admin: bool,
    #[sea_orm(not_null)]
    pub created_at: i64,
    pub last_login_at: Option<i64>,
    /// User who invited this user (FK → users.id). None for the admin/root user.
    pub invited_by: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::api_token::Entity")]
    ApiTokens,
    #[sea_orm(has_many = "super::repo::Entity")]
    Repos,
    #[sea_orm(has_many = "super::repo_member::Entity")]
    RepoMembers,
    #[sea_orm(has_many = "super::sync_token::Entity")]
    SyncTokens,
    #[sea_orm(has_one = "super::user_2fa::Entity")]
    User2fa,
    #[sea_orm(has_many = "super::s2fa_token::Entity")]
    S2faTokens,
    #[sea_orm(has_many = "super::user_2fa_backup_code::Entity")]
    User2faBackupCodes,
    #[sea_orm(has_many = "super::invitation_code::Entity")]
    InvitationCodesCreated,
    #[sea_orm(has_many = "super::invitation_code::Entity")]
    InvitationCodesUsed,
    #[sea_orm(has_many = "super::share_link::Entity")]
    ShareLinks,
    #[sea_orm(has_many = "super::upload_link::Entity")]
    UploadLinks,
}

impl Related<super::api_token::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ApiTokens.def()
    }
}

impl Related<super::repo::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Repos.def()
    }
}

impl Related<super::repo_member::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RepoMembers.def()
    }
}

impl Related<super::sync_token::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SyncTokens.def()
    }
}

impl Related<super::user_2fa::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User2fa.def()
    }
}

impl Related<super::user_2fa_backup_code::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User2faBackupCodes.def()
    }
}

impl Related<super::s2fa_token::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::S2faTokens.def()
    }
}

impl Related<super::share_link::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ShareLinks.def()
    }
}

impl Related<super::upload_link::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::UploadLinks.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
