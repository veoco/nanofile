use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(FileLockTimestamps::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FileLockTimestamps::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(FileLockTimestamps::RepoId)
                            .string_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FileLockTimestamps::UpdateTime)
                            .big_integer()
                            .not_null(),
                    )
                    .index(
                        Index::create()
                            .unique()
                            .name("idx_file_lock_timestamps_repo_id")
                            .col(FileLockTimestamps::RepoId),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(FileLockTimestamps::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum FileLockTimestamps {
    Table,
    Id,
    RepoId,
    UpdateTime,
}
