use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SyncTokens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SyncTokens::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(SyncTokens::Token)
                            .char_len(40)
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(SyncTokens::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SyncTokens::UserId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SyncTokens::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(SyncTokens::ExpiresAt).big_integer())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_sync_tokens_repo_id")
                            .from(SyncTokens::Table, SyncTokens::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_sync_tokens_user_id")
                            .from(SyncTokens::Table, SyncTokens::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SyncTokens::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum SyncTokens {
    Table,
    Id,
    Token,
    RepoId,
    UserId,
    CreatedAt,
    ExpiresAt,
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
