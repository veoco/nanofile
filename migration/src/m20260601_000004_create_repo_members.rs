use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RepoMembers::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RepoMembers::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(RepoMembers::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepoMembers::UserId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RepoMembers::Permission)
                            .char_len(2)
                            .not_null()
                            .default("rw"),
                    )
                    .col(
                        ColumnDef::new(RepoMembers::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_repo_members_repo_id")
                            .from(RepoMembers::Table, RepoMembers::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_repo_members_user_id")
                            .from(RepoMembers::Table, RepoMembers::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_repo_members_unique")
                    .table(RepoMembers::Table)
                    .col(RepoMembers::RepoId)
                    .col(RepoMembers::UserId)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RepoMembers::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum RepoMembers {
    Table,
    Id,
    RepoId,
    UserId,
    Permission,
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
