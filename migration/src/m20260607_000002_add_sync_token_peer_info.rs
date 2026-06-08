use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add peer_id (client_id) — the device's ccnet ID, used to link
        // sync tokens to a specific device for remote unlink.
        manager
            .alter_table(
                Table::alter()
                    .table(SyncTokens::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(SyncTokens::PeerId).string_len(40),
                    )
                    .to_owned(),
            )
            .await?;

        // Add peer_name — the device's human-readable name (e.g. "my-laptop").
        manager
            .alter_table(
                Table::alter()
                    .table(SyncTokens::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(SyncTokens::PeerName).string_len(255),
                    )
                    .to_owned(),
            )
            .await?;

        // Add peer_ip — the device's IP address during sync.
        manager
            .alter_table(
                Table::alter()
                    .table(SyncTokens::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(SyncTokens::PeerIp).string_len(50),
                    )
                    .to_owned(),
            )
            .await?;

        // Add client_version — the seafile client version string.
        manager
            .alter_table(
                Table::alter()
                    .table(SyncTokens::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(SyncTokens::ClientVersion).string_len(20),
                    )
                    .to_owned(),
            )
            .await?;

        // Add last_sync_time — timestamp of most recent sync request.
        manager
            .alter_table(
                Table::alter()
                    .table(SyncTokens::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(SyncTokens::LastSyncTime).big_integer(),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite does not support DROP COLUMN in older versions.
        // These columns are optional (nullable), so removal is not critical.
        // If the SQLite version supports it (3.35.0+), uncomment:
        // manager
        //     .alter_table(
        //         Table::alter()
        //             .table(SyncTokens::Table)
        //             .drop_column(SyncTokens::PeerId)
        //             .to_owned(),
        //     )
        //     .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum SyncTokens {
    Table,
    PeerId,
    PeerName,
    PeerIp,
    ClientVersion,
    LastSyncTime,
}
