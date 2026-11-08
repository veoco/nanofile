//! Repository traits and implementations for data access.
//!
//! Each module defines a trait interface and a `Db*` implementation over SeaORM queries, which
//! decouples service code from the persistence framework and lets business logic be tested with mocks.

pub mod activity;
pub mod api_key;
pub mod api_token;
pub mod auth_cache;
pub mod avatar;
pub mod client_login_token;
pub mod commit;
pub mod deleted_repo;
pub mod email_message;
pub mod email_verification_token;
pub mod face_tags;
pub mod file_lock_timestamp;
pub mod file_tag;
pub mod file_trash;
pub mod fs_object;
pub mod invitation_code;
pub mod job_run;
pub mod locked_file;
pub mod member;
pub mod metadata_config;
pub mod metadata_record;
pub mod password_reset_token;
pub mod repo;
pub mod repo_tag;
pub mod s2fa_token;
pub mod settings;
pub mod share_link;
pub mod sso_login_token;
pub mod starred;
pub mod sync_token;
pub mod throttle;
pub mod thumbnail;
pub mod token_cleanup;
pub mod upload_link;
pub mod user;
pub mod user_2fa;
pub mod user_2fa_backup_code;

use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::repository::activity::*;
use crate::repository::api_key::*;
use crate::repository::api_token::*;
use crate::repository::avatar::*;
use crate::repository::client_login_token::*;
use crate::repository::commit::*;
use crate::repository::deleted_repo::*;
use crate::repository::email_message::*;
use crate::repository::email_verification_token::*;
use crate::repository::face_tags::*;
use crate::repository::file_lock_timestamp::*;
use crate::repository::file_tag::*;
use crate::repository::file_trash::*;
use crate::repository::fs_object::*;
use crate::repository::invitation_code::*;
use crate::repository::job_run::*;
use crate::repository::locked_file::*;
use crate::repository::member::*;
use crate::repository::metadata_config::*;
use crate::repository::metadata_record::*;
use crate::repository::password_reset_token::*;
use crate::repository::repo::*;
use crate::repository::repo_tag::*;
use crate::repository::s2fa_token::*;
use crate::repository::settings::*;
use crate::repository::share_link::*;
use crate::repository::sso_login_token::*;
use crate::repository::starred::*;
use crate::repository::sync_token::*;
use crate::repository::thumbnail::*;
use crate::repository::upload_link::*;
use crate::repository::user::*;
use crate::repository::user_2fa::*;
use crate::repository::user_2fa_backup_code::*;

use base::error::AppError;

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
    pub thumbnail: Arc<dyn ThumbnailRepository>,
    pub avatar: Arc<dyn AvatarRepository>,
    pub invitation_code: Arc<dyn InvitationCodeRepository>,
    pub locked_file: Arc<dyn LockedFileRepository>,
    pub sync_token: Arc<dyn SyncTokenRepository>,
    pub api_token: Arc<dyn ApiTokenRepository>,
    pub api_key: Arc<dyn ApiKeyRepository>,
    pub s2fa_token: Arc<dyn S2faTokenRepository>,
    pub user_2fa: Arc<dyn User2faRepository>,
    pub sso_login_token: Arc<dyn SsoLoginTokenRepository>,
    pub client_login_token: Arc<dyn ClientLoginTokenRepository>,
    pub metadata_config: Arc<dyn MetadataConfigRepository>,
    pub metadata_record: Arc<dyn MetadataRecordRepository>,
    pub repo_tag: Arc<dyn RepoTagRepository>,
    pub file_tag: Arc<dyn FileTagRepository>,
    pub face_tags: Arc<dyn FaceTagRepository>,
    pub file_trash: Arc<dyn FileTrashRepository>,
    pub deleted_repo: Arc<dyn DeletedRepoRepository>,
    pub settings: Arc<dyn SettingsRepository>,
    pub email_message: Arc<dyn EmailMessageRepository>,
    pub job_run: Arc<dyn JobRunRepository>,
    pub password_reset_token: Arc<dyn PasswordResetTokenRepository>,
    pub email_verification_token: Arc<dyn EmailVerificationTokenRepository>,
    pub user_2fa_backup_code: Arc<dyn User2faBackupCodeRepository>,
    pub file_lock_timestamp: Arc<dyn FileLockTimestampRepository>,
    /// Per-server quota accounting state (committed-usage snapshots and uncommitted-block
    /// reservations). Owned here, not in process-global statics, so it describes this database.
    pub quota_cache: Arc<infra::quota_cache::QuotaCache>,
    /// At most one write per interval for the two "when was this last used?" columns the auth path
    /// updates; on the database handle because they describe writes to *this* database (see [`throttle`]).
    pub peer_info_writes: throttle::WriteThrottle,
    pub key_usage_writes: throttle::WriteThrottle,
}

impl std::fmt::Debug for Repositories {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repositories").finish_non_exhaustive()
    }
}

impl Repositories {
    pub fn new(
        db: Arc<DatabaseConnection>,
        token_cipher: Arc<infra::crypto::token_encryption::TokenCipher>,
        totp_cipher: Arc<infra::crypto::totp_encryption::TotpCipher>,
    ) -> Self {
        // The same cipher is shared by the sync-token store and the SSO flow; both need a
        // recoverable (not hashed) server-side token.
        let sso_token_cipher = token_cipher.clone();
        Self {
            peer_info_writes: throttle::WriteThrottle::new(throttle::PEER_INFO_WRITE_INTERVAL),
            key_usage_writes: throttle::WriteThrottle::new(throttle::KEY_USAGE_WRITE_INTERVAL),
            user: Arc::new(DbUserRepository::new(db.clone())),
            repo: Arc::new(DbRepoRepository::new(db.clone())),
            member: Arc::new(DbMemberRepository::new(db.clone())),
            commit: Arc::new(DbCommitRepository::new(db.clone())),
            share_link: Arc::new(DbShareLinkRepository::new(db.clone())),
            starred: Arc::new(DbStarredRepository::new(db.clone())),
            upload_link: Arc::new(DbUploadLinkRepository::new(db.clone())),
            fs_object: Arc::new(DbFsObjectRepository::new(db.clone())),
            activity: Arc::new(DbActivityRepository::new(db.clone())),
            thumbnail: Arc::new(DbThumbnailRepository::new(db.clone())),
            avatar: Arc::new(DbAvatarRepository::new(db.clone())),
            invitation_code: Arc::new(DbInvitationCodeRepository::new(db.clone())),
            locked_file: Arc::new(DbLockedFileRepository::new(db.clone())),
            sync_token: Arc::new(auth_cache::CachingSyncTokenRepository::new(Arc::new(
                DbSyncTokenRepository::new(db.clone(), token_cipher.clone()),
            ))),
            api_token: Arc::new(auth_cache::CachingApiTokenRepository::new(Arc::new(
                DbApiTokenRepository::new(db.clone()),
            ))),
            api_key: Arc::new(auth_cache::CachingApiKeyRepository::new(Arc::new(
                DbApiKeyRepository::new(db.clone()),
            ))),
            s2fa_token: Arc::new(DbS2faTokenRepository::new(db.clone())),
            user_2fa: Arc::new(DbUser2faRepository::new(db.clone(), totp_cipher)),
            sso_login_token: Arc::new(DbSsoLoginTokenRepository::new(db.clone(), sso_token_cipher)),
            client_login_token: Arc::new(DbClientLoginTokenRepository::new(db.clone())),
            metadata_config: Arc::new(DbMetadataConfigRepository::new(db.clone())),
            metadata_record: Arc::new(DbMetadataRecordRepository::new(db.clone())),
            repo_tag: Arc::new(DbRepoTagRepository::new(db.clone())),
            file_tag: Arc::new(DbFileTagRepository::new(db.clone())),
            face_tags: Arc::new(DbFaceTagRepository::new(db.clone())),
            file_trash: Arc::new(DbFileTrashRepository::new(db.clone())),
            deleted_repo: Arc::new(DbDeletedRepoRepository::new(db.clone())),
            settings: Arc::new(DbSettingsRepository::new(db.clone())),
            email_message: Arc::new(DbEmailMessageRepository::new(db.clone())),
            job_run: Arc::new(DbJobRunRepository::new(db.clone())),
            password_reset_token: Arc::new(DbPasswordResetTokenRepository::new(db.clone())),
            email_verification_token: Arc::new(DbEmailVerificationTokenRepository::new(db.clone())),
            user_2fa_backup_code: Arc::new(DbUser2faBackupCodeRepository::new(db.clone())),
            file_lock_timestamp: Arc::new(DbFileLockTimestampRepository::new(db.clone())),
            quota_cache: Arc::new(infra::quota_cache::QuotaCache::new()),
        }
    }

    /// Test-only constructor: derives the repository ciphers from an empty master key. Production
    /// must call [`Repositories::new`] with ciphers derived from the configured server secret.
    #[cfg(test)]
    pub fn new_for_tests(db: Arc<DatabaseConnection>) -> Self {
        Self::new(
            db,
            Arc::new(infra::crypto::token_encryption::TokenCipher::from_master_key(b"")),
            Arc::new(infra::crypto::totp_encryption::TotpCipher::from_master_key(
                b"",
            )),
        )
    }

    /// Total storage usage for a user, summed in SQL rather than by pulling every repo row.
    pub async fn compute_user_usage(&self, user_id: i32) -> Result<i64, AppError> {
        self.repo.sum_size_by_owner(user_id).await
    }

    /// The owner of a library. Current libraries are single-user, but quota checks
    /// still go through this instead of assuming the caller is the owner, so a
    /// future sharing feature cannot silently bill the wrong account.
    pub async fn repo_owner_id(&self, repo_id: &str) -> Result<i32, AppError> {
        self.repo
            .find_by_id(repo_id)
            .await?
            .map(|r| r.owner_id)
            .ok_or_else(|| AppError::NotFound("repo not found".into()))
    }

    /// Resolve a user's effective quota: their own column, or the server default
    /// when it is NULL. `Some(0)` (and any non-positive stored value) means
    /// unlimited; the default is also unlimited when it is 0.
    pub async fn effective_user_quota(
        &self,
        user_id: i32,
        default_quota: u64,
    ) -> Result<Option<i64>, AppError> {
        let user = self
            .user
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;
        let quota = match user.storage_quota {
            Some(0) => return Ok(None), // explicitly unlimited
            Some(n) => n,
            None => default_quota as i64, // follows the server default
        };
        if quota <= 0 {
            return Ok(None);
        }
        Ok(Some(quota))
    }

    /// Serialize quota accounting for one user. The caller must hold the returned
    /// scope across the quota check and the commit that spends the checked headroom.
    pub async fn lock_quota(&self, user_id: i32) -> infra::quota_cache::QuotaScope {
        self.quota_cache.lock_user(user_id).await
    }

    /// Check a positive logical delta against the **owner's** freshly summed usage.
    /// Must be called while holding the owner's `QuotaScope`, otherwise two commits
    /// can both observe the pre-commit total.
    pub async fn check_repo_quota_locked(
        &self,
        scope: &infra::quota_cache::QuotaScope,
        repo_id: &str,
        delta: i64,
        default_quota: u64,
    ) -> Result<(), AppError> {
        if delta <= 0 {
            return Ok(());
        }
        let owner = self.repo_owner_id(repo_id).await?;
        self.check_user_quota_locked(scope, owner, delta, default_quota)
            .await
    }

    /// The repo-less half of the check, for operations that allocate bytes before
    /// the library row exists (restoring a deleted library, for example).
    pub async fn check_user_quota_locked(
        &self,
        scope: &infra::quota_cache::QuotaScope,
        user_id: i32,
        delta: i64,
        default_quota: u64,
    ) -> Result<(), AppError> {
        if delta <= 0 {
            return Ok(());
        }
        if user_id != scope.user_id() {
            return Err(AppError::internal(
                "quota scope does not match the account being charged",
            ));
        }
        let Some(quota) = self.effective_user_quota(user_id, default_quota).await? else {
            return Ok(());
        };
        let usage = self.compute_user_usage(user_id).await?;
        if usage + delta > quota {
            return Err(AppError::QuotaExceeded);
        }
        Ok(())
    }

    /// Drop the cached usage snapshot for a user after a committed size change.
    pub fn invalidate_quota(&self, user_id: i32) {
        self.quota_cache.invalidate_usage(user_id);
    }
}
