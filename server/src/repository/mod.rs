//! Repository traits and implementations for data access.
//!
//! Each module defines a trait interface and a `Db*` implementation that
//! wraps the underlying SeaORM entity queries. This decouples service-layer
//! code from the persistence framework and entity module, making it easier
//! to test business logic with mock repositories and to split into
//! base/infra crates (Phase 4).

pub mod activity;
pub mod api_token;
pub mod avatar;
pub mod client_login_token;
pub mod commit;
pub mod deleted_repo;
pub mod file_lock_timestamp;
pub mod file_tag;
pub mod fs_object;
pub mod group;
pub mod group_member;
pub mod invitation_code;
pub mod locked_file;
pub mod member;
pub mod metadata_config;
pub mod metadata_record;
pub mod password_reset_token;
pub mod repo;
pub mod s2fa_token;
pub mod sdoc_comment;
pub mod share_link;
pub mod sso_login_token;
pub mod starred;
pub mod sync_token;
pub mod thumbnail;
pub mod upload_link;
pub mod user;
pub mod user_2fa;
pub mod user_2fa_backup_code;
pub mod user_contact;
pub mod wiki;

pub use activity::*;
pub use api_token::*;
pub use avatar::*;
pub use client_login_token::*;
pub use commit::*;
pub use deleted_repo::*;
pub use file_lock_timestamp::*;
pub use file_tag::*;
pub use fs_object::*;
pub use group::*;
pub use group_member::*;
pub use invitation_code::*;
pub use locked_file::*;
pub use member::*;
pub use metadata_config::*;
pub use metadata_record::*;
pub use password_reset_token::*;
pub use repo::*;
pub use s2fa_token::*;
pub use sdoc_comment::*;
pub use share_link::*;
pub use sso_login_token::*;
pub use starred::*;
pub use sync_token::*;
pub use thumbnail::*;
pub use upload_link::*;
pub use user::*;
pub use user_2fa::*;
pub use user_2fa_backup_code::*;
pub use user_contact::*;
pub use wiki::*;

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
    pub deleted_repo: Arc<dyn DeletedRepoRepository>,
    pub password_reset_token: Arc<dyn PasswordResetTokenRepository>,
    pub user_2fa_backup_code: Arc<dyn User2faBackupCodeRepository>,
    pub file_lock_timestamp: Arc<dyn FileLockTimestampRepository>,
    pub sdoc_comment: Arc<dyn SdocCommentRepository>,
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
            deleted_repo: Arc::new(DbDeletedRepoRepository::new(db.clone())),
            password_reset_token: Arc::new(DbPasswordResetTokenRepository::new(db.clone())),
            user_2fa_backup_code: Arc::new(DbUser2faBackupCodeRepository::new(db.clone())),
            file_lock_timestamp: Arc::new(DbFileLockTimestampRepository::new(db.clone())),
            sdoc_comment: Arc::new(DbSdocCommentRepository::new(db.clone())),
        }
    }
}
