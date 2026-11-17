use sea_orm::entity::prelude::*;

/// A small key/value row for state that has no better home: currently the time of
/// the last audit pass, so an incremental schedule can do a full scan periodically
/// without adding a second background job.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "gc_state")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,
    pub value: String,
    pub updated_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
