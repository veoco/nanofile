use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // metadata_config — per-repo metadata configuration
        manager
            .create_table(
                Table::create()
                    .table(MetadataConfig::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MetadataConfig::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(MetadataConfig::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MetadataConfig::Enabled)
                            .boolean()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(MetadataConfig::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_mc_repo_id")
                            .from(MetadataConfig::Table, MetadataConfig::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // metadata_records — key-value metadata for files
        manager
            .create_table(
                Table::create()
                    .table(MetadataRecords::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MetadataRecords::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(MetadataRecords::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(ColumnDef::new(MetadataRecords::FilePath).text().not_null())
                    .col(
                        ColumnDef::new(MetadataRecords::RecordKey)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(ColumnDef::new(MetadataRecords::RecordValue).text())
                    .col(
                        ColumnDef::new(MetadataRecords::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MetadataRecords::UpdatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_mr_repo_id")
                            .from(MetadataRecords::Table, MetadataRecords::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // file_tags — tags applied to files
        manager
            .create_table(
                Table::create()
                    .table(FileTags::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FileTags::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(FileTags::RepoId).char_len(36).not_null())
                    .col(ColumnDef::new(FileTags::FilePath).text().not_null())
                    .col(ColumnDef::new(FileTags::TagName).string_len(64).not_null())
                    .col(ColumnDef::new(FileTags::CreatedAt).big_integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_ft_repo_id")
                            .from(FileTags::Table, FileTags::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(FileTags::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(MetadataRecords::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(MetadataConfig::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum MetadataConfig {
    Table,
    Id,
    RepoId,
    Enabled,
    CreatedAt,
}
#[derive(Iden)]
enum MetadataRecords {
    Table,
    Id,
    RepoId,
    FilePath,
    RecordKey,
    RecordValue,
    CreatedAt,
    UpdatedAt,
}
#[derive(Iden)]
enum FileTags {
    Table,
    Id,
    RepoId,
    FilePath,
    TagName,
    CreatedAt,
}
#[derive(Iden)]
enum Repos {
    Table,
    Id,
}
