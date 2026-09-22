use sea_orm::entity::prelude::*;

/// One saved setting (`key = "auth.max_login_attempts"`), in the canonical
/// string form the catalog parses.
///
/// The table is deliberately untyped about values: `infra::settings::CATALOG`
/// owns which keys exist, how each is parsed, and whether a change needs a
/// restart. Adding a setting is therefore a code change with no migration, and
/// the database can never disagree with the code about a key's type.
///
/// Secret values are stored encrypted (`enc1:` prefix, the same AEAD cipher as
/// repository sync tokens), so a leaked database does not yield a usable SMTP
/// password — which is exactly how the row this table replaces behaved.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
#[sea_orm(table_name = "settings")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,
    #[sea_orm(not_null, default_value = "")]
    pub value: String,
    #[sea_orm(not_null, default_value = 0)]
    pub updated_at: i64,
    /// Admin who last saved the value. No foreign key: deleting an account must
    /// not touch (or fail on) a setting it once changed.
    #[sea_orm(nullable)]
    pub updated_by: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
