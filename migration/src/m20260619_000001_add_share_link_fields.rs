use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add s_type, view_cnt, description columns to share_links
        manager
            .alter_table(
                Table::alter()
                    .table(ShareLinks::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(ShareLinks::SType)
                            .char_len(1)
                            .not_null()
                            .default("f"),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(ShareLinks::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(ShareLinks::ViewCnt)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(ShareLinks::Table)
                    .add_column_if_not_exists(ColumnDef::new(ShareLinks::Description).text())
                    .to_owned(),
            )
            .await?;

        // Add view_cnt, description columns to upload_links
        manager
            .alter_table(
                Table::alter()
                    .table(UploadLinks::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(UploadLinks::ViewCnt)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(UploadLinks::Table)
                    .add_column_if_not_exists(ColumnDef::new(UploadLinks::Description).text())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ShareLinks::Table)
                    .drop_column(ShareLinks::SType)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ShareLinks::Table)
                    .drop_column(ShareLinks::ViewCnt)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ShareLinks::Table)
                    .drop_column(ShareLinks::Description)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(UploadLinks::Table)
                    .drop_column(UploadLinks::ViewCnt)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(UploadLinks::Table)
                    .drop_column(UploadLinks::Description)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum ShareLinks {
    Table,
    SType,
    ViewCnt,
    Description,
}

#[derive(Iden)]
enum UploadLinks {
    Table,
    ViewCnt,
    Description,
}
