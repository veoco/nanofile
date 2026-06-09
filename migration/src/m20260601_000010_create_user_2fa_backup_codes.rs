use sea_orm_migration::prelude::*;
use std::fmt;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(User2faBackupCodes::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(User2faBackupCodes::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(User2faBackupCodes::UserId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(User2faBackupCodes::CodeHash)
                            .char_len(64)
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(User2faBackupCodes::Used)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(User2faBackupCodes::UsedAt).big_integer())
                    .col(
                        ColumnDef::new(User2faBackupCodes::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_2fa_backup_codes_user_id")
                            .from(User2faBackupCodes::Table, User2faBackupCodes::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(User2faBackupCodes::Table).to_owned())
            .await
    }
}

enum User2faBackupCodes {
    Table,
    Id,
    UserId,
    CodeHash,
    Used,
    UsedAt,
    CreatedAt,
}

impl Iden for User2faBackupCodes {
    fn unquoted(&self, s: &mut dyn fmt::Write) {
        match self {
            Self::Table => write!(s, "user_2fa_backup_codes").unwrap(),
            Self::Id => write!(s, "id").unwrap(),
            Self::UserId => write!(s, "user_id").unwrap(),
            Self::CodeHash => write!(s, "code_hash").unwrap(),
            Self::Used => write!(s, "used").unwrap(),
            Self::UsedAt => write!(s, "used_at").unwrap(),
            Self::CreatedAt => write!(s, "created_at").unwrap(),
        }
    }
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
