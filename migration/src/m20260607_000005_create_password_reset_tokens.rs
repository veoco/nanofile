use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PasswordResetTokens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PasswordResetTokens::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(PasswordResetTokens::UserId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PasswordResetTokens::TokenHash)
                            .char_len(64)
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(PasswordResetTokens::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PasswordResetTokens::ExpiresAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PasswordResetTokens::Used)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_password_reset_tokens_user_id")
                            .from(
                                PasswordResetTokens::Table,
                                PasswordResetTokens::UserId,
                            )
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PasswordResetTokens::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum PasswordResetTokens {
    Table,
    Id,
    UserId,
    TokenHash,
    CreatedAt,
    ExpiresAt,
    Used,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
