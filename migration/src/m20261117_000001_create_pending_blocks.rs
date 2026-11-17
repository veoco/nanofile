use sea_orm_migration::prelude::*;

/// The durable "written but not committed" ledger, and the small state row an
/// incremental garbage-collection schedule needs.
///
/// `pending_blocks` replaces an in-memory-only reservation for the purpose of
/// finding an interrupted upload's blocks after a restart: the row is written
/// before the block, deleted by the commit (or the abort), and expired by garbage
/// collection. `gc_state` currently holds the time of the last full audit, so the
/// scheduled pass can scan periodically without a second job.
///
/// Both are new tables: an existing deployment starts empty, and blocks that a
/// crash left behind before the upgrade are still found by the audit pass.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PendingBlocks::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PendingBlocks::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingBlocks::BlockId)
                            .char_len(40)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PendingBlocks::WrittenAt)
                            .big_integer()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(PendingBlocks::RepoId)
                            .col(PendingBlocks::BlockId),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_pending_blocks_written_at")
                    .table(PendingBlocks::Table)
                    .col(PendingBlocks::WrittenAt)
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(GcState::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(GcState::Key)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(GcState::Value).text().not_null())
                    .col(ColumnDef::new(GcState::UpdatedAt).big_integer().not_null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(GcState::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(PendingBlocks::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum PendingBlocks {
    Table,
    RepoId,
    BlockId,
    WrittenAt,
}

#[derive(DeriveIden)]
enum GcState {
    Table,
    Key,
    Value,
    UpdatedAt,
}
