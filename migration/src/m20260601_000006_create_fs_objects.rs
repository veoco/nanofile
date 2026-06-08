use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(FsObjects::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FsObjects::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(FsObjects::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FsObjects::FsId)
                            .char_len(40)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FsObjects::ObjType)
                            .tiny_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FsObjects::Data)
                            .binary()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_fs_objects_repo_id")
                            .from(FsObjects::Table, FsObjects::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_fs_objects_repo_fs")
                    .table(FsObjects::Table)
                    .col(FsObjects::RepoId)
                    .col(FsObjects::FsId)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(FsObjects::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum FsObjects {
    Table,
    Id,
    RepoId,
    FsId,
    ObjType,
    Data,
}

#[derive(Iden)]
enum Repos {
    Table,
    Id,
}
