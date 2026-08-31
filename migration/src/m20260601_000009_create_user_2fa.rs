use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(User2fa::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(User2fa::UserId)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(User2fa::TotpSecret)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(User2fa::Algorithm)
                            .char_len(6)
                            .not_null()
                            .default("SHA1"),
                    )
                    .col(
                        ColumnDef::new(User2fa::Digits)
                            .small_integer()
                            .not_null()
                            .default(6),
                    )
                    .col(
                        ColumnDef::new(User2fa::Period)
                            .small_integer()
                            .not_null()
                            .default(30),
                    )
                    .col(
                        ColumnDef::new(User2fa::Enabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(User2fa::EnabledAt).big_integer())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_2fa_user_id")
                            .from(User2fa::Table, User2fa::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(User2fa::Table).to_owned())
            .await
    }
}

enum User2fa {
    Table,
    UserId,
    TotpSecret,
    Algorithm,
    Digits,
    Period,
    Enabled,
    EnabledAt,
}

impl Iden for User2fa {
    fn unquoted(&self) -> &str {
        match self {
            Self::Table => "user_2fa",
            Self::UserId => "user_id",
            Self::TotpSecret => "totp_secret",
            Self::Algorithm => "algorithm",
            Self::Digits => "digits",
            Self::Period => "period",
            Self::Enabled => "enabled",
            Self::EnabledAt => "enabled_at",
        }
    }
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
