use sea_orm::entity::prelude::*;

/// One block that an upload has written but no commit references yet.
///
/// The row is inserted before the write, so a crash between the two leaves a row
/// pointing at a block that may not exist (garbage collection deletes a missing
/// block idempotently). A deduplicated write and a successful commit both delete
/// the row. The table is the durable replacement for the in-memory reservation the
/// quota cache keeps: it is what lets garbage collection find an interrupted
/// upload's blocks after a restart, and what answers "is this block newer than the
/// grace window" without asking the store.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "pending_blocks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, length = 36)]
    pub repo_id: String,
    #[sea_orm(primary_key, auto_increment = false, length = 40)]
    pub block_id: String,
    /// Unix epoch seconds, taken by the server when the write was announced.
    pub written_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
