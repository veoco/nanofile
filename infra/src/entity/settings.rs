use sea_orm::entity::prelude::*;

/// One saved setting (`key = "auth.max_login_attempts"`) in the canonical string form
/// the catalog parses.
///
/// The table is untyped about values: `infra::settings::CATALOG` owns which keys exist
/// and how they parse, so adding a setting needs no migration. Secrets are stored
/// encrypted (`enc1:`, the sync-token AEAD), so a leaked database yields no usable
/// SMTP password.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
#[sea_orm(table_name = "settings")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,
    #[sea_orm(not_null, default_value = "")]
    pub value: String,
    #[sea_orm(not_null, default_value = 0)]
    pub updated_at: i64,
    /// Admin who last saved it. No foreign key: deleting an account must not fail here.
    #[sea_orm(nullable)]
    pub updated_by: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
