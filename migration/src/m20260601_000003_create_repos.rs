use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Repos::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Repos::Id)
                            .char_len(36)
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Repos::Name)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Repos::Description)
                            .string_len(1024)
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(Repos::OwnerId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Repos::Encrypted)
                            .tiny_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Repos::EncVersion)
                            .tiny_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(Repos::Magic).string_len(64))
                    .col(ColumnDef::new(Repos::RandomKey).string_len(96))
                    .col(
                        ColumnDef::new(Repos::Salt)
                            .string_len(255)
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(Repos::HeadCommitId).char_len(40))
                    .col(
                        ColumnDef::new(Repos::Permission)
                            .char_len(2)
                            .not_null()
                            .default("rw"),
                    )
                    .col(
                        ColumnDef::new(Repos::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Repos::UpdatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_repos_owner_id")
                            .from(Repos::Table, Repos::OwnerId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Repos::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum Repos {
    Table,
    Id,
    Name,
    Description,
    OwnerId,
    Encrypted,
    EncVersion,
    Magic,
    RandomKey,
    Salt,
    HeadCommitId,
    Permission,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
