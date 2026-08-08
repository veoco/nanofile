use sea_orm_migration::prelude::*;

/// File tags redesign for seafile metadata-service compatibility.
///
/// Adds the `repo_tags` table (tag definitions with name+color), rebuilds
/// `file_tags` to reference `repo_tags.id` instead of storing a bare tag
/// name, and adds a `tags_enabled` flag to `metadata_config`.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // repo_tags — tag definitions (name + color), scoped to a repo.
        manager
            .create_table(
                Table::create()
                    .table(RepoTags::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepoTags::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(RepoTags::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(ColumnDef::new(RepoTags::Name).string_len(255).not_null())
                    .col(
                        ColumnDef::new(RepoTags::Color)
                            .string_len(64)
                            .not_null()
                            .default("#e6e6e6"),
                    )
                    .col(
                        ColumnDef::new(RepoTags::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_rt_repo_id")
                            .from(RepoTags::Table, RepoTags::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_repo_tags_repo_name")
                    .table(RepoTags::Table)
                    .col(RepoTags::RepoId)
                    .col(RepoTags::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Rebuild file_tags: drop the old flat (repo_id, file_path, tag_name)
        // layout and recreate it referencing repo_tags.
        manager
            .drop_table(Table::drop().table(FileTags::Table).if_exists().to_owned())
            .await?;
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
                    .col(ColumnDef::new(FileTags::RepoTagId).integer().not_null())
                    .col(ColumnDef::new(FileTags::CreatedAt).big_integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_ft_repo_id")
                            .from(FileTags::Table, FileTags::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_ft_repo_tag_id")
                            .from(FileTags::Table, FileTags::RepoTagId)
                            .to(RepoTags::Table, RepoTags::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_file_tags_repo_path")
                    .table(FileTags::Table)
                    .col(FileTags::RepoId)
                    .col(FileTags::FilePath)
                    .to_owned(),
            )
            .await?;

        // metadata_config — add tags_enabled flag (default on).
        manager
            .alter_table(
                Table::alter()
                    .table(MetadataConfig::Table)
                    .add_column(
                        ColumnDef::new(MetadataConfig::TagsEnabled)
                            .boolean()
                            .default(true),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(MetadataConfig::Table)
                    .drop_column(MetadataConfig::TagsEnabled)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(FileTags::Table).if_exists().to_owned())
            .await?;
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
            .await?;

        manager
            .drop_table(Table::drop().table(RepoTags::Table).if_exists().to_owned())
            .await
    }
}

#[derive(Iden)]
enum RepoTags {
    Table,
    Id,
    RepoId,
    Name,
    Color,
    CreatedAt,
}

#[derive(Iden)]
enum FileTags {
    Table,
    Id,
    RepoId,
    FilePath,
    RepoTagId,
    TagName,
    CreatedAt,
}

#[derive(Iden)]
enum MetadataConfig {
    Table,
    TagsEnabled,
}

#[derive(Iden)]
enum Repos {
    Table,
    Id,
}
