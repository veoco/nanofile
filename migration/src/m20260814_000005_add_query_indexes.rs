use sea_orm_migration::prelude::*;

/// Add indexes for hot query paths that currently scan whole tables:
/// - `commits.commit_id` (commit lookup by id)
/// - `commits(repo_id, ctime)` (repo history ordering)
/// - `repo_members.user_id` (repos shared with a user)
/// - `group_members.user_id`
/// - `share_links.creator_id` / `upload_links.creator_id`
/// - `api_tokens.device_id` (device removal)
/// - `metadata_records(repo_id, file_path, record_key)` (currently zero indexes)
/// - `metadata_config.repo_id`
/// - `file_tags.repo_tag_id` (tag file listing)
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx_commits_commit_id")
                    .table("commits")
                    .col("commit_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_commits_repo_ctime")
                    .table("commits")
                    .col("repo_id")
                    .col("ctime")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_repo_members_user_id")
                    .table("repo_members")
                    .col("user_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_group_members_user_id")
                    .table("group_members")
                    .col("user_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_share_links_creator_id")
                    .table("share_links")
                    .col("creator_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_upload_links_creator_id")
                    .table("upload_links")
                    .col("creator_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_api_tokens_device_id")
                    .table("api_tokens")
                    .col("device_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_metadata_records_repo_path_key")
                    .table("metadata_records")
                    .col("repo_id")
                    .col("file_path")
                    .col("record_key")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_metadata_config_repo_id")
                    .table("metadata_config")
                    .col("repo_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_file_tags_repo_tag_id")
                    .table("file_tags")
                    .col("repo_tag_id")
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("idx_file_tags_repo_tag_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_metadata_config_repo_id").to_owned())
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_metadata_records_repo_path_key")
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(Index::drop().name("idx_api_tokens_device_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_upload_links_creator_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_share_links_creator_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_group_members_user_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_repo_members_user_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_commits_repo_ctime").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_commits_commit_id").to_owned())
            .await
    }
}
