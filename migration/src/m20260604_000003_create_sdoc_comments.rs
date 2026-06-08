use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SdocComments::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SdocComments::Id).integer().not_null().auto_increment().primary_key())
                    .col(ColumnDef::new(SdocComments::DocUuid).char_len(36).not_null())
                    .col(ColumnDef::new(SdocComments::UserId).integer().not_null())
                    .col(ColumnDef::new(SdocComments::Content).text().not_null())
                    .col(ColumnDef::new(SdocComments::Resolved).boolean().default(false))
                    .col(ColumnDef::new(SdocComments::CreatedAt).big_integer().not_null())
                    .foreign_key(
                        ForeignKey::create().name("fk_sdoc_user_id")
                            .from(SdocComments::Table, SdocComments::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(SdocComments::Table).to_owned()).await
    }
}

#[derive(Iden)]
enum SdocComments { Table, Id, DocUuid, UserId, Content, Resolved, CreatedAt }
#[derive(Iden)]
enum Users { Table, Id }

