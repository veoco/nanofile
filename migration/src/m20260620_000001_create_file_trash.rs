use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(FileTrash::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FileTrash::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(FileTrash::User).string_len(255).not_null())
                    .col(
                        ColumnDef::new(FileTrash::ObjType)
                            .string_len(128)
                            .not_null(),
                    )
                    .col(ColumnDef::new(FileTrash::ObjId).char_len(40).not_null())
                    .col(
                        ColumnDef::new(FileTrash::ObjName)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FileTrash::DeleteTime)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(FileTrash::RepoId).char_len(36).not_null())
                    .col(ColumnDef::new(FileTrash::CommitId).char_len(40).not_null())
                    .col(ColumnDef::new(FileTrash::Path).text().not_null())
                    .col(
                        ColumnDef::new(FileTrash::Size)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;

        // Index for trash listing (by repo_id, ordered by delete_time DESC)
        manager
            .create_index(
                Index::create()
                    .name("idx_file_trash_repo_time")
                    .table(FileTrash::Table)
                    .col(FileTrash::RepoId)
                    .col(FileTrash::DeleteTime)
                    .to_owned(),
            )
            .await?;

        // Index for trash search (by repo_id + obj_name LIKE)
        manager
            .create_index(
                Index::create()
                    .name("idx_file_trash_repo_name")
                    .table(FileTrash::Table)
                    .col(FileTrash::RepoId)
                    .col(FileTrash::ObjName)
                    .to_owned(),
            )
            .await?;

        // Index for restore lookup (by repo_id + commit_id + path + obj_name)
        manager
            .create_index(
                Index::create()
                    .name("idx_file_trash_repo_commit_path")
                    .table(FileTrash::Table)
                    .col(FileTrash::RepoId)
                    .col(FileTrash::CommitId)
                    .col(FileTrash::Path)
                    .col(FileTrash::ObjName)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(FileTrash::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum FileTrash {
    Table,
    Id,
    User,
    ObjType,
    ObjId,
    ObjName,
    DeleteTime,
    RepoId,
    CommitId,
    Path,
    Size,
}
