//! Repository traits and implementations for data access.
//!
//! Each module defines a trait interface and a `Db*` implementation that
//! wraps the underlying SeaORM entity queries. This decouples service-layer
//! code from the persistence framework and entity module, making it easier
//! to test business logic with mock repositories and to split into
//! domain/infra crates (Phase 4).

pub mod activity_repository;
pub mod api_token_repository;
pub mod avatar_repository;
pub mod client_login_token_repository;
pub mod commit_repository;
pub mod file_tag_repository;
pub mod fs_object_repository;
pub mod group_member_repository;
pub mod group_repository;
pub mod invitation_code_repository;
pub mod locked_file_repository;
pub mod member_repository;
pub mod metadata_config_repository;
pub mod metadata_record_repository;
pub mod repo_repository;
pub mod s2fa_token_repository;
pub mod share_link_repository;
pub mod sso_login_token_repository;
pub mod starred_repository;
pub mod sync_token_repository;
pub mod thumbnail_repository;
pub mod upload_link_repository;
pub mod user_2fa_repository;
pub mod user_contact_repository;
pub mod user_repository;
pub mod wiki_repository;

pub use activity_repository::*;
pub use api_token_repository::*;
pub use avatar_repository::*;
pub use client_login_token_repository::*;
pub use commit_repository::*;
pub use file_tag_repository::*;
pub use fs_object_repository::*;
pub use group_member_repository::*;
pub use group_repository::*;
pub use invitation_code_repository::*;
pub use locked_file_repository::*;
pub use member_repository::*;
pub use metadata_config_repository::*;
pub use metadata_record_repository::*;
pub use repo_repository::*;
pub use s2fa_token_repository::*;
pub use share_link_repository::*;
pub use sso_login_token_repository::*;
pub use starred_repository::*;
pub use sync_token_repository::*;
pub use thumbnail_repository::*;
pub use upload_link_repository::*;
pub use user_2fa_repository::*;
pub use user_contact_repository::*;
pub use user_repository::*;
pub use wiki_repository::*;

use std::sync::Arc;

use sea_orm::DatabaseConnection;

/// Bundles all repository implementations for convenient injection into services.
pub struct Repositories {
    pub user: Arc<dyn UserRepository>,
    pub repo: Arc<dyn RepoRepository>,
    pub member: Arc<dyn MemberRepository>,
    pub commit: Arc<dyn CommitRepository>,
    pub share_link: Arc<dyn ShareLinkRepository>,
    pub starred: Arc<dyn StarredRepository>,
    pub upload_link: Arc<dyn UploadLinkRepository>,
    pub fs_object: Arc<dyn FsObjectRepository>,
    pub activity: Arc<dyn ActivityRepository>,
    pub group: Arc<dyn GroupRepository>,
    pub group_member: Arc<dyn GroupMemberRepository>,
    pub user_contact: Arc<dyn UserContactRepository>,
    pub wiki: Arc<dyn WikiRepository>,
    pub thumbnail: Arc<dyn ThumbnailRepository>,
    pub avatar: Arc<dyn AvatarRepository>,
    pub invitation_code: Arc<dyn InvitationCodeRepository>,
    pub locked_file: Arc<dyn LockedFileRepository>,
    pub sync_token: Arc<dyn SyncTokenRepository>,
    pub api_token: Arc<dyn ApiTokenRepository>,
    pub s2fa_token: Arc<dyn S2faTokenRepository>,
    pub user_2fa: Arc<dyn User2faRepository>,
    pub sso_login_token: Arc<dyn SsoLoginTokenRepository>,
    pub client_login_token: Arc<dyn ClientLoginTokenRepository>,
    pub metadata_config: Arc<dyn MetadataConfigRepository>,
    pub metadata_record: Arc<dyn MetadataRecordRepository>,
    pub file_tag: Arc<dyn FileTagRepository>,
}

impl Repositories {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self {
            user: Arc::new(DbUserRepository::new(db.clone())),
            repo: Arc::new(DbRepoRepository::new(db.clone())),
            member: Arc::new(DbMemberRepository::new(db.clone())),
            commit: Arc::new(DbCommitRepository::new(db.clone())),
            share_link: Arc::new(DbShareLinkRepository::new(db.clone())),
            starred: Arc::new(DbStarredRepository::new(db.clone())),
            upload_link: Arc::new(DbUploadLinkRepository::new(db.clone())),
            fs_object: Arc::new(DbFsObjectRepository::new(db.clone())),
            activity: Arc::new(DbActivityRepository::new(db.clone())),
            group: Arc::new(DbGroupRepository::new(db.clone())),
            group_member: Arc::new(DbGroupMemberRepository::new(db.clone())),
            user_contact: Arc::new(DbUserContactRepository::new(db.clone())),
            wiki: Arc::new(DbWikiRepository::new(db.clone())),
            thumbnail: Arc::new(DbThumbnailRepository::new(db.clone())),
            avatar: Arc::new(DbAvatarRepository::new(db.clone())),
            invitation_code: Arc::new(DbInvitationCodeRepository::new(db.clone())),
            locked_file: Arc::new(DbLockedFileRepository::new(db.clone())),
            sync_token: Arc::new(DbSyncTokenRepository::new(db.clone())),
            api_token: Arc::new(DbApiTokenRepository::new(db.clone())),
            s2fa_token: Arc::new(DbS2faTokenRepository::new(db.clone())),
            user_2fa: Arc::new(DbUser2faRepository::new(db.clone())),
            sso_login_token: Arc::new(DbSsoLoginTokenRepository::new(db.clone())),
            client_login_token: Arc::new(DbClientLoginTokenRepository::new(db.clone())),
            metadata_config: Arc::new(DbMetadataConfigRepository::new(db.clone())),
            metadata_record: Arc::new(DbMetadataRecordRepository::new(db.clone())),
            file_tag: Arc::new(DbFileTagRepository::new(db.clone())),
        }
    }
}
