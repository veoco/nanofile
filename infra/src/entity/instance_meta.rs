use sea_orm::entity::prelude::*;

/// Server-wide key/value state that is not a user setting.
///
/// The settings table is driven by the settings catalog; this table is for the
/// few values the server writes about itself (currently the fingerprint of the
/// master secret the stored sessions belong to).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "instance_meta")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,
    #[sea_orm(not_null)]
    pub value: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
