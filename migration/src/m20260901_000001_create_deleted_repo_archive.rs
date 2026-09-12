use sea_orm_migration::prelude::*;

/// Archive of a deleted library's content.
///
/// Deleting a library removes its `repos` row, which cascades into `commits`
/// and `fs_objects` — the commit graph and every FS object (directory listings
/// and file block lists) of that library. Without a copy, restoring a library
/// from the trash can only rebuild an empty repository, even though its blocks
/// are still on disk.
///
/// These two tables hold that copy while a library sits in the trash, mirroring
/// the official server, which moves a deleted library's commit and FS stores to
/// `seafile-data/deleted_store/` and keeps its blocks until the trash entry is
/// purged. There is deliberately **no** foreign key to `repos`: the whole point
/// is to outlive the repo row, and the row is gone by the time these are read.
///
/// The archived columns mirror the `commit` and `fs_object` entities. The
/// legacy `commits.encrypted` column is not copied: no code reads it.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DeletedRepoCommits::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DeletedRepoCommits::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoCommits::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoCommits::CommitId)
                            .char_len(40)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoCommits::RootId)
                            .char_len(40)
                            .not_null(),
                    )
                    .col(ColumnDef::new(DeletedRepoCommits::ParentId).char_len(40).null())
                    .col(
                        ColumnDef::new(DeletedRepoCommits::SecondParentId)
                            .char_len(40)
                            .null(),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoCommits::CreatorName)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoCommits::Creator)
                            .char_len(40)
                            .not_null()
                            .default("0000000000000000000000000000000000000000"),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoCommits::Description)
                            .string_len(4096)
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoCommits::Ctime)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoCommits::Version)
                            .tiny_integer()
                            .not_null()
                            .default(1),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_deleted_repo_commits_repo_commit")
                    .table(DeletedRepoCommits::Table)
                    .col(DeletedRepoCommits::RepoId)
                    .col(DeletedRepoCommits::CommitId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(DeletedRepoFsObjects::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DeletedRepoFsObjects::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoFsObjects::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoFsObjects::FsId)
                            .char_len(40)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoFsObjects::ObjType)
                            .tiny_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeletedRepoFsObjects::Data)
                            .binary()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_deleted_repo_fs_objects_repo_fs")
                    .table(DeletedRepoFsObjects::Table)
                    .col(DeletedRepoFsObjects::RepoId)
                    .col(DeletedRepoFsObjects::FsId)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DeletedRepoFsObjects::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(DeletedRepoCommits::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum DeletedRepoCommits {
    Table,
    Id,
    RepoId,
    CommitId,
    RootId,
    ParentId,
    SecondParentId,
    CreatorName,
    Creator,
    Description,
    Ctime,
    Version,
}

#[derive(Iden)]
enum DeletedRepoFsObjects {
    Table,
    Id,
    RepoId,
    FsId,
    ObjType,
    Data,
}
