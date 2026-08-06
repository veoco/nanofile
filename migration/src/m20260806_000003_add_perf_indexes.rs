use sea_orm_migration::prelude::*;

/// Add indexes for hot query paths that currently scan whole tables:
/// - `activities.user_id` (activity list per user)
/// - `api_tokens.user_id` (device list)
/// - `share_links.expires_at` / `upload_links.expires_at` (hourly cleanup)
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Activities are filtered/sorted by user (find_recent_by_user).
        manager
            .create_index(
                Index::create()
                    .name("idx_activities_user_created")
                    .table(Activities::Table)
                    .col(Activities::UserId)
                    .col(Activities::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // Device listing groups tokens by user.
        manager
            .create_index(
                Index::create()
                    .name("idx_api_tokens_user_id")
                    .table(ApiTokens::Table)
                    .col(ApiTokens::UserId)
                    .to_owned(),
            )
            .await?;

        // Hourly cleanup of expired share/upload links scans `expires_at`.
        manager
            .create_index(
                Index::create()
                    .name("idx_share_links_expires_at")
                    .table(ShareLinks::Table)
                    .col(ShareLinks::ExpiresAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_upload_links_expires_at")
                    .table(UploadLinks::Table)
                    .col(UploadLinks::ExpiresAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("idx_upload_links_expires_at").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_share_links_expires_at").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_api_tokens_user_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_activities_user_created").to_owned())
            .await
    }
}

#[derive(Iden)]
enum Activities {
    Table,
    UserId,
    CreatedAt,
}

#[derive(Iden)]
enum ApiTokens {
    Table,
    UserId,
}

#[derive(Iden)]
enum ShareLinks {
    Table,
    ExpiresAt,
}

#[derive(Iden)]
enum UploadLinks {
    Table,
    ExpiresAt,
}
