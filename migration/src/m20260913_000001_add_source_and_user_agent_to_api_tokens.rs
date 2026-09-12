use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite rejects several alter options in one statement, so the columns
        // are added one at a time.
        manager
            .alter_table(
                Table::alter()
                    .table(ApiTokens::Table)
                    .add_column(
                        ColumnDef::new(ApiTokens::Source)
                            .string()
                            .not_null()
                            // Any row this build did not write is assumed to be
                            // a browser session: the default also backfills the
                            // existing rows, which is the safe direction — a
                            // session shown in the wrong group is still visible
                            // and revocable, whereas a hidden one is neither.
                            .default("web"),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(ApiTokens::Table)
                    .add_column(ColumnDef::new(ApiTokens::UserAgent).string().null())
                    .to_owned(),
            )
            .await?;

        // `platform` used to be the only hint of where a token came from, and it
        // is only set when the client reported device details. Rows that have it
        // were client logins; the rest keep the `web` default.
        manager
            .get_connection()
            .execute_unprepared("UPDATE api_tokens SET source = 'client' WHERE platform IS NOT NULL")
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [ApiTokens::UserAgent, ApiTokens::Source] {
            manager
                .alter_table(
                    Table::alter()
                        .table(ApiTokens::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(Iden)]
enum ApiTokens {
    Table,
    Source,
    UserAgent,
}
