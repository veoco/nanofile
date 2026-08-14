use sea_orm_migration::prelude::*;

/// Drop the legacy `wikis` table. The new wiki2 model marks a library as a
/// wiki via `repos.type = 'wiki'` (m20260814_000001), so this standalone table
/// is no longer used and has no production data (fresh feature).
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Wikis::Table).if_exists().to_owned())
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Re-create the legacy table (informational; not used by the new model).
        manager
            .create_table(
                Table::create()
                    .table(Wikis::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Wikis::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Wikis::RepoId).char_len(36).not_null())
                    .col(ColumnDef::new(Wikis::Name).string_len(255).not_null())
                    .col(ColumnDef::new(Wikis::OwnerId).integer().not_null())
                    .col(ColumnDef::new(Wikis::Published).boolean().default(false))
                    .col(
                        ColumnDef::new(Wikis::Permission)
                            .string_len(10)
                            .default("private"),
                    )
                    .col(ColumnDef::new(Wikis::CreatedAt).big_integer().not_null())
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum Wikis {
    Table,
    Id,
    RepoId,
    Name,
    OwnerId,
    Published,
    Permission,
    CreatedAt,
}
