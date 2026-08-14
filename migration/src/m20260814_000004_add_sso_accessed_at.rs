use sea_orm_migration::prelude::*;

/// Add the columns the SSO local-browser flow needs:
/// - `accessed_at`: when the browser first opened `/client-sso/{token}/`.
///   The seahub-compatible soft timeout is measured from this timestamp, not
///   from creation time.
/// - `client_version`: the desktop client's version (from the `shib_*` query
///   params) so the token minted on completion can be device-bound.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite only supports one alter option per ALTER TABLE statement.
        manager
            .alter_table(
                Table::alter()
                    .table(SsoLoginTokens::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(SsoLoginTokens::AccessedAt).big_integer(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SsoLoginTokens::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(SsoLoginTokens::ClientVersion).string_len(64),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SsoLoginTokens::Table)
                    .drop_column(SsoLoginTokens::AccessedAt)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SsoLoginTokens::Table)
                    .drop_column(SsoLoginTokens::ClientVersion)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum SsoLoginTokens {
    Table,
    AccessedAt,
    ClientVersion,
}
