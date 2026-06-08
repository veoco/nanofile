use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(InvitationCodes::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(InvitationCodes::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(InvitationCodes::Code)
                            .char_len(32)
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(InvitationCodes::CreatorId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(InvitationCodes::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(InvitationCodes::UsedBy).integer())
                    .col(ColumnDef::new(InvitationCodes::UsedAt).big_integer())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_invitation_codes_creator_id")
                            .from(InvitationCodes::Table, InvitationCodes::CreatorId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_invitation_codes_used_by")
                            .from(InvitationCodes::Table, InvitationCodes::UsedBy)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(InvitationCodes::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum InvitationCodes {
    Table,
    Id,
    Code,
    CreatorId,
    CreatedAt,
    UsedBy,
    UsedAt,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
