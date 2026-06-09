use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_wikis_repo_id")
                            .from(Wikis::Table, Wikis::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_wikis_owner_id")
                            .from(Wikis::Table, Wikis::OwnerId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Wikis::Table).to_owned())
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
