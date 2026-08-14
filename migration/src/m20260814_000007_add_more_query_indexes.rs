use sea_orm_migration::prelude::*;

/// Add indexes for remaining unindexed query paths:
/// - `s2fa_tokens.user_id` (expired-token cleanup and per-device removal)
/// - `repos.type` (wiki repo listing)
/// - `password_reset_tokens.user_id` (cleanup)
/// - `user_2fa_backup_codes.user_id` (backup-code lookup)
/// - `user_contacts.user_id` (contact listing)
/// - `invitation_codes.creator_id` (invitation listing)
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx_s2fa_tokens_user_id")
                    .table("s2fa_tokens")
                    .col("user_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_repos_type")
                    .table("repos")
                    .col("type")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_password_reset_tokens_user_id")
                    .table("password_reset_tokens")
                    .col("user_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_user_2fa_backup_codes_user_id")
                    .table("user_2fa_backup_codes")
                    .col("user_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_user_contacts_user_id")
                    .table("user_contacts")
                    .col("user_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_invitation_codes_creator_id")
                    .table("invitation_codes")
                    .col("creator_id")
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("idx_invitation_codes_creator_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_user_contacts_user_id").to_owned())
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_user_2fa_backup_codes_user_id")
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_password_reset_tokens_user_id")
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(Index::drop().name("idx_repos_type").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_s2fa_tokens_user_id").to_owned())
            .await
    }
}
