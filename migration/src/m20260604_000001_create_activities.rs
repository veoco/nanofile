use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Activities::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Activities::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Activities::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Activities::CommitId)
                            .char_len(40)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Activities::OpType)
                            .char_len(32)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Activities::ObjType)
                            .char_len(16)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Activities::Path).text().not_null())
                    .col(
                        ColumnDef::new(Activities::UserId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Activities::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_activities_repo_id")
                            .from(Activities::Table, Activities::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_activities_user_id")
                            .from(Activities::Table, Activities::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Index on (repo_id, created_at) for efficient activity queries
        manager
            .create_index(
                Index::create()
                    .name("idx_activities_repo_created")
                    .table(Activities::Table)
                    .col(Activities::RepoId)
                    .col(Activities::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Activities::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum Activities {
    Table,
    Id,
    RepoId,
    CommitId,
    OpType,
    ObjType,
    Path,
    UserId,
    CreatedAt,
}

#[derive(Iden)]
enum Repos {
    Table,
    Id,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
