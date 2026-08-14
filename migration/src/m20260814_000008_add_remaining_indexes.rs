use sea_orm_migration::prelude::*;

/// Add composite indexes for query paths whose secondary sort/filter column is
/// still unindexed (so the primary filter column hits an index but the trailing
/// ORDER BY / filter falls back to a sort or scan):
/// - `api_tokens(user_id, created_at)` (device listing order)
/// - `commits(repo_id, ctime, id)` (history order with same-second tiebreaker)
/// - `repo_tags(repo_id, created_at)` (tag listing order)
/// - `invitation_codes(creator_id, created_at)` (invitation listing order)
/// - `webdav_keys(repo_id, created_at)` (per-repo key listing order)
/// - `file_tags(repo_id, repo_tag_id)` (list files by repo+tag)
/// - `share_links(repo_id, path)` / `upload_links(repo_id, path)` (link lookup)
/// - `s2fa_tokens(user_id, expires_at)` (expired-token cleanup)
/// - `sync_tokens(user_id, peer_id)` (remote unlink by peer)
/// - `repos(owner_id, type)` (wiki repo lookup by owner)
///
/// `idx_commits_repo_ctime` is superseded by `idx_commits_repo_ctime_id`
/// (its `(repo_id, ctime)` prefix) and dropped here.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx_api_tokens_user_created")
                    .table("api_tokens")
                    .col("user_id")
                    .col("created_at")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_commits_repo_ctime_id")
                    .table("commits")
                    .col("repo_id")
                    .col("ctime")
                    .col("id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_repo_tags_repo_created")
                    .table("repo_tags")
                    .col("repo_id")
                    .col("created_at")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_invitation_codes_creator_created")
                    .table("invitation_codes")
                    .col("creator_id")
                    .col("created_at")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_webdav_keys_repo_created")
                    .table("webdav_keys")
                    .col("repo_id")
                    .col("created_at")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_file_tags_repo_repo_tag")
                    .table("file_tags")
                    .col("repo_id")
                    .col("repo_tag_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_share_links_repo_path")
                    .table("share_links")
                    .col("repo_id")
                    .col("path")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_upload_links_repo_path")
                    .table("upload_links")
                    .col("repo_id")
                    .col("path")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_s2fa_tokens_user_expires")
                    .table("s2fa_tokens")
                    .col("user_id")
                    .col("expires_at")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_sync_tokens_user_peer")
                    .table("sync_tokens")
                    .col("user_id")
                    .col("peer_id")
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_repos_owner_type")
                    .table("repos")
                    .col("owner_id")
                    .col("type")
                    .to_owned(),
            )
            .await?;

        // Superseded by idx_commits_repo_ctime_id.
        manager
            .drop_index(Index::drop().name("idx_commits_repo_ctime").to_owned())
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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
            .drop_index(Index::drop().name("idx_repos_owner_type").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_sync_tokens_user_peer").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_s2fa_tokens_user_expires").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_upload_links_repo_path").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_share_links_repo_path").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_file_tags_repo_repo_tag").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_webdav_keys_repo_created").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_invitation_codes_creator_created").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_repo_tags_repo_created").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_commits_repo_ctime_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_api_tokens_user_created").to_owned())
            .await
    }
}
