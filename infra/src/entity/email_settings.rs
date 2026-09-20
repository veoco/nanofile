use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// The single outbound-mail settings row (`id = 1`).
///
/// The master switch is deliberately *not* here: `[email] enabled` in
/// `config.toml` decides whether the mail subsystem exists at all, so a
/// database row can never turn mail on by itself. Everything else about the
/// SMTP connection lives in this row so an administrator can change it from
/// `/sysadmin/email/` without a restart.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "email_settings")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Stop delivery without touching the config switch. A paused server still
    /// mints reset tokens (the recipient can retry later) but sends nothing.
    #[sea_orm(not_null, default_value = false)]
    pub paused: bool,
    #[sea_orm(not_null, default_value = "")]
    pub host: String,
    #[sea_orm(not_null, default_value = 587)]
    pub port: i32,
    /// `"starttls"` | `"tls"` (implicit, port 465) | `"none"`.
    #[sea_orm(not_null, default_value = "starttls")]
    pub tls: String,
    #[sea_orm(not_null, default_value = "")]
    pub username: String,
    /// SMTP password, encrypted at rest with the domain-separated token cipher.
    #[sea_orm(nullable)]
    pub password_enc: Option<String>,
    #[sea_orm(not_null, default_value = "")]
    pub from_address: String,
    #[sea_orm(not_null, default_value = "")]
    pub from_name: String,
    #[sea_orm(not_null, default_value = 10)]
    pub timeout_secs: i32,
    #[sea_orm(not_null, default_value = 5)]
    pub max_attempts: i32,
    #[sea_orm(not_null, default_value = true)]
    pub notify_new_device: bool,
    #[sea_orm(not_null, default_value = true)]
    pub notify_api_key_created: bool,
    #[sea_orm(not_null, default_value = true)]
    pub notify_new_login: bool,
    #[sea_orm(not_null, default_value = 0)]
    pub updated_at: i64,
    /// Admin who last saved the settings. No foreign key: deleting an account
    /// must not touch (or fail on) the settings row.
    #[sea_orm(nullable)]
    pub updated_by: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
