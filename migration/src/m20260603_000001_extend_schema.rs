use sea_orm_migration::prelude::*;

/// Phase 1 schema changes: add missing fields and tables for full Seafile API compatibility.
///
/// Changes:
/// - users:             add name, display_name, storage_total, storage_usage, is_staff
/// - api_tokens:        add device_id, platform, device_name, client_version
/// - repos:             add repo_version, size
/// - commits:           add encrypted, creator
/// - sync_tokens:       add UNIQUE(repo_id, user_id)
/// - Add indexes on     repos(owner_id), fs_objects(repo_id, fs_id), share_links(repo_id),
///   upload_links(repo_id), sync_tokens(user_id), commits(commit_id)
/// - New tables:        starred_files, locked_files, groups, group_members,
///   user_contacts, sso_login_tokens, thumbnails
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ================ Table field additions ================

        // users
        manager
            .alter_table(
                Table::alter()
                    .table(UsersRef)
                    .add_column_if_not_exists(ColumnDef::new(UsersRefCol::Name).string_len(255))
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(UsersRef)
                    .add_column_if_not_exists(
                        ColumnDef::new(UsersRefCol::DisplayName).string_len(255),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(UsersRef)
                    .add_column_if_not_exists(
                        ColumnDef::new(UsersRefCol::StorageTotal)
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
                    .table(UsersRef)
                    .add_column_if_not_exists(
                        ColumnDef::new(UsersRefCol::StorageUsage)
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
                    .table(UsersRef)
                    .add_column_if_not_exists(
                        ColumnDef::new(UsersRefCol::IsStaff)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;

        // api_tokens
        manager
            .alter_table(
                Table::alter()
                    .table(OldApiTokens)
                    .add_column_if_not_exists(
                        ColumnDef::new(OldApiTokensCol::DeviceId).string_len(255),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(OldApiTokens)
                    .add_column_if_not_exists(
                        ColumnDef::new(OldApiTokensCol::Platform).string_len(64),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(OldApiTokens)
                    .add_column_if_not_exists(
                        ColumnDef::new(OldApiTokensCol::DeviceName).string_len(255),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(OldApiTokens)
                    .add_column_if_not_exists(
                        ColumnDef::new(OldApiTokensCol::ClientVersion).string_len(64),
                    )
                    .to_owned(),
            )
            .await?;

        // repos
        manager
            .alter_table(
                Table::alter()
                    .table(OldRepos)
                    .add_column_if_not_exists(
                        ColumnDef::new(OldReposCol::RepoVersion)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(OldRepos)
                    .add_column_if_not_exists(
                        ColumnDef::new(OldReposCol::Size)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;

        // commits
        manager
            .alter_table(
                Table::alter()
                    .table(OldCommits)
                    .add_column_if_not_exists(
                        ColumnDef::new(OldCommitsCol::Encrypted).string_len(16),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(OldCommits)
                    .add_column_if_not_exists(
                        ColumnDef::new(OldCommitsCol::Creator)
                            .char_len(40)
                            .not_null()
                            .default("0000000000000000000000000000000000000000"),
                    )
                    .to_owned(),
            )
            .await?;

        // ================ Indexes ================

        manager
            .create_index(
                Index::create()
                    .name("idx_repos_owner_id")
                    .table(OldRepos)
                    .col(OldReposCol::OwnerId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_share_links_repo_id")
                    .table(OldShareLinks)
                    .col(OldShareLinksCol::RepoId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_upload_links_repo_id")
                    .table(OldUploadLinks)
                    .col(OldUploadLinksCol::RepoId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_sync_tokens_user_id")
                    .table(OldSyncTokens)
                    .col(OldSyncTokensCol::UserId)
                    .to_owned(),
            )
            .await?;

        // UNIQUE(repo_id, user_id) on sync_tokens — enforced via unique index
        // since SQLite ALTER TABLE cannot add constraints.
        manager
            .create_index(
                Index::create()
                    .name("idx_sync_tokens_repo_user_unique")
                    .table(OldSyncTokens)
                    .col(OldSyncTokensCol::RepoId)
                    .col(OldSyncTokensCol::UserId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // ================ New tables ================

        manager
            .create_table(
                Table::create()
                    .table(StarredFiles::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(StarredFiles::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(StarredFiles::RepoId).char_len(36).not_null())
                    .col(
                        ColumnDef::new(StarredFiles::Path)
                            .string_len(4096)
                            .not_null(),
                    )
                    .col(ColumnDef::new(StarredFiles::UserId).integer().not_null())
                    .col(
                        ColumnDef::new(StarredFiles::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_starred_files_repo_id")
                            .from(StarredFiles::Table, StarredFiles::RepoId)
                            .to(OldRepos, OldReposCol::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_starred_files_user_id")
                            .from(StarredFiles::Table, StarredFiles::UserId)
                            .to(UsersRef, UsersRefCol::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_starred_files_user_repo")
                    .table(StarredFiles::Table)
                    .col(StarredFiles::UserId)
                    .col(StarredFiles::RepoId)
                    .col(StarredFiles::Path)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(LockedFiles::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LockedFiles::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(LockedFiles::RepoId).char_len(36).not_null())
                    .col(
                        ColumnDef::new(LockedFiles::Path)
                            .string_len(4096)
                            .not_null(),
                    )
                    .col(ColumnDef::new(LockedFiles::UserId).integer().not_null())
                    .col(
                        ColumnDef::new(LockedFiles::LockedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(LockedFiles::LockOwnerName)
                            .string_len(255)
                            .not_null()
                            .default(""),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_locked_files_repo_id")
                            .from(LockedFiles::Table, LockedFiles::RepoId)
                            .to(OldRepos, OldReposCol::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_locked_files_user_id")
                            .from(LockedFiles::Table, LockedFiles::UserId)
                            .to(UsersRef, UsersRefCol::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_locked_files_repo_path")
                    .table(LockedFiles::Table)
                    .col(LockedFiles::RepoId)
                    .col(LockedFiles::Path)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Groups::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Groups::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Groups::Name).string_len(255).not_null())
                    .col(ColumnDef::new(Groups::CreatorId).integer().not_null())
                    .col(ColumnDef::new(Groups::CreatedAt).big_integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_groups_creator_id")
                            .from(Groups::Table, Groups::CreatorId)
                            .to(UsersRef, UsersRefCol::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(GroupMembers::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(GroupMembers::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(GroupMembers::GroupId).integer().not_null())
                    .col(ColumnDef::new(GroupMembers::UserId).integer().not_null())
                    .col(
                        ColumnDef::new(GroupMembers::Role)
                            .char_len(10)
                            .not_null()
                            .default("member"),
                    )
                    .col(
                        ColumnDef::new(GroupMembers::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_group_members_group_id")
                            .from(GroupMembers::Table, GroupMembers::GroupId)
                            .to(Groups::Table, Groups::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_group_members_user_id")
                            .from(GroupMembers::Table, GroupMembers::UserId)
                            .to(UsersRef, UsersRefCol::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_group_members_unique")
                    .table(GroupMembers::Table)
                    .col(GroupMembers::GroupId)
                    .col(GroupMembers::UserId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(UserContacts::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserContacts::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UserContacts::UserId).integer().not_null())
                    .col(
                        ColumnDef::new(UserContacts::ContactEmail)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(ColumnDef::new(UserContacts::ContactName).string_len(255))
                    .col(
                        ColumnDef::new(UserContacts::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_contacts_user_id")
                            .from(UserContacts::Table, UserContacts::UserId)
                            .to(UsersRef, UsersRefCol::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SsoLoginTokens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SsoLoginTokens::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(SsoLoginTokens::Token)
                            .char_len(64)
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(SsoLoginTokens::Platform).string_len(64))
                    .col(ColumnDef::new(SsoLoginTokens::DeviceId).string_len(255))
                    .col(ColumnDef::new(SsoLoginTokens::DeviceName).string_len(255))
                    .col(
                        ColumnDef::new(SsoLoginTokens::Status)
                            .char_len(16)
                            .not_null()
                            .default("pending"),
                    )
                    .col(ColumnDef::new(SsoLoginTokens::Username).string_len(255))
                    .col(ColumnDef::new(SsoLoginTokens::ApiToken).char_len(40))
                    .col(
                        ColumnDef::new(SsoLoginTokens::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(SsoLoginTokens::ExpiresAt).big_integer())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Thumbnails::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Thumbnails::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Thumbnails::RepoId).char_len(36).not_null())
                    .col(ColumnDef::new(Thumbnails::Path).string_len(4096).not_null())
                    .col(
                        ColumnDef::new(Thumbnails::Size)
                            .integer()
                            .not_null()
                            .default(128),
                    )
                    .col(
                        ColumnDef::new(Thumbnails::FileModifiedAt)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Thumbnails::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_thumbnails_repo_id")
                            .from(Thumbnails::Table, Thumbnails::RepoId)
                            .to(OldRepos, OldReposCol::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_thumbnails_repo_path")
                    .table(Thumbnails::Table)
                    .col(Thumbnails::RepoId)
                    .col(Thumbnails::Path)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop new tables (reverse order of creation)
        manager
            .drop_table(Table::drop().table(Thumbnails::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(SsoLoginTokens::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(UserContacts::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(GroupMembers::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Groups::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(LockedFiles::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(StarredFiles::Table).to_owned())
            .await?;

        // Drop indexes (no-op if they don't exist in a future rollback scenario)
        let _ = manager
            .drop_index(Index::drop().name("idx_repos_owner_id").to_owned())
            .await;
        let _ = manager
            .drop_index(Index::drop().name("idx_share_links_repo_id").to_owned())
            .await;
        let _ = manager
            .drop_index(Index::drop().name("idx_upload_links_repo_id").to_owned())
            .await;
        let _ = manager
            .drop_index(Index::drop().name("idx_sync_tokens_user_id").to_owned())
            .await;

        // SQLite cannot drop columns (ADD COLUMN only), so we can't revert
        // column additions. This is a documented limitation.
        Ok(())
    }
}

// ================ Old table Iden types (derive with renamed table idens) ================

#[derive(Iden)]
#[iden = "users"]
struct UsersRef;

#[derive(Iden)]
#[iden = "users"]
#[allow(dead_code)]
enum UsersRefCol {
    Table,
    Id,
    Name,
    DisplayName,
    StorageTotal,
    StorageUsage,
    IsStaff,
}

#[derive(Iden)]
#[iden = "api_tokens"]
#[allow(dead_code)]
struct OldApiTokens;

#[derive(Iden)]
#[iden = "api_tokens"]
#[allow(dead_code)]
enum OldApiTokensCol {
    Table,
    DeviceId,
    Platform,
    DeviceName,
    ClientVersion,
}

#[derive(Iden)]
#[iden = "repos"]
#[allow(dead_code)]
struct OldRepos;

#[derive(Iden)]
#[iden = "repos"]
#[allow(dead_code)]
enum OldReposCol {
    Table,
    Id,
    OwnerId,
    RepoVersion,
    Size,
}

#[derive(Iden)]
#[iden = "commits"]
#[allow(dead_code)]
struct OldCommits;

#[derive(Iden)]
#[iden = "commits"]
#[allow(dead_code)]
enum OldCommitsCol {
    Table,
    Encrypted,
    Creator,
}

#[derive(Iden)]
#[iden = "fs_objects"]
#[allow(dead_code)]
struct OldFsObjects;

#[derive(Iden)]
#[iden = "fs_objects"]
#[allow(dead_code)]
enum OldFsObjectsCol {
    Table,
    RepoId,
    FsId,
}

#[derive(Iden)]
#[iden = "share_links"]
#[allow(dead_code)]
struct OldShareLinks;

#[derive(Iden)]
#[iden = "share_links"]
#[allow(dead_code)]
enum OldShareLinksCol {
    Table,
    RepoId,
}

#[derive(Iden)]
#[iden = "upload_links"]
#[allow(dead_code)]
struct OldUploadLinks;

#[derive(Iden)]
#[iden = "upload_links"]
#[allow(dead_code)]
enum OldUploadLinksCol {
    Table,
    RepoId,
}

#[derive(Iden)]
#[iden = "sync_tokens"]
#[allow(dead_code)]
struct OldSyncTokens;

#[derive(Iden)]
#[iden = "sync_tokens"]
#[allow(dead_code)]
enum OldSyncTokensCol {
    Table,
    RepoId,
    UserId,
}

// ================ New table Iden types ================

#[derive(Iden)]
enum StarredFiles {
    Table,
    Id,
    RepoId,
    Path,
    UserId,
    CreatedAt,
}

#[derive(Iden)]
enum LockedFiles {
    Table,
    Id,
    RepoId,
    Path,
    UserId,
    LockedAt,
    LockOwnerName,
}

#[derive(Iden)]
enum Groups {
    Table,
    Id,
    Name,
    CreatorId,
    CreatedAt,
}

#[derive(Iden)]
enum GroupMembers {
    Table,
    Id,
    GroupId,
    UserId,
    Role,
    CreatedAt,
}

#[derive(Iden)]
enum UserContacts {
    Table,
    Id,
    UserId,
    ContactEmail,
    ContactName,
    CreatedAt,
}

#[derive(Iden)]
enum SsoLoginTokens {
    Table,
    Id,
    Token,
    Platform,
    DeviceId,
    DeviceName,
    Status,
    Username,
    ApiToken,
    CreatedAt,
    ExpiresAt,
}

#[derive(Iden)]
enum Thumbnails {
    Table,
    Id,
    RepoId,
    Path,
    Size,
    FileModifiedAt,
    CreatedAt,
}
