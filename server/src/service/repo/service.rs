use sea_orm::DatabaseConnection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::AccessTokenManager;
use crate::repository::Repositories;
use crate::service::auth::token::{ensure_sync_token, generate_sync_token};
use base::error::AppError;
use infra::activity_log;
use infra::common::util::{format_size, timestamp_rfc3339};
use infra::entity::{repo, user};

// ── Response types ──────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct RepoInfo {
    pub id: String,
    pub name: String,
    pub desc: String,
    #[serde(rename = "owner")]
    pub owner: String,
    pub encrypted: bool,
    #[serde(rename = "enc_version", skip_serializing_if = "Option::is_none")]
    pub enc_version: Option<i32>,
    pub size: i64,
    pub mtime: i64,
    #[serde(rename = "permission")]
    pub permission: String,
    #[serde(rename = "head_commit_id")]
    pub head_commit_id: Option<String>,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(rename = "virtual")]
    pub virtual_: bool,
    pub root: Option<String>,
    pub salt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub random_key: Option<String>,
    pub repo_version: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lib_need_decrypt: Option<bool>,
    #[serde(rename = "repo_id", skip_serializing_if = "Option::is_none")]
    pub repo_id_dup: Option<String>,
    #[serde(rename = "repo_name", skip_serializing_if = "Option::is_none")]
    pub repo_name_dup: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Serialize)]
pub struct DownloadInfoResponse {
    pub repo_id: String,
    pub repo_name: String,
    pub token: String,
    pub email: String,
    pub relay_id: Option<String>,
    pub relay_addr: Option<String>,
    pub relay_port: Option<String>,
    pub enc_version: i32,
    pub encrypted: String,
    pub magic: Option<String>,
    pub random_key: Option<String>,
    pub repo_version: i32,
    pub salt: Option<String>,
    pub permission: String,
}

#[derive(Serialize)]
pub struct V21RepoListResponse {
    pub repos: Vec<V21RepoInfo>,
}

#[derive(Serialize)]
pub struct V21RepoInfo {
    pub repo_id: String,
    pub repo_name: String,
    pub repo_desc: String,
    pub permission: String,
    pub encrypted: bool,
    #[serde(rename = "type")]
    pub type_: String,
    pub size: i64,
    pub last_modified: String,
    pub mtime: i64,
    pub owner_email: String,
    pub owner_name: String,
}

// ── Service ─────────────────────────────────────────────────────────────

pub struct RepoService;

fn build_op_url(site_url: &str, op: &str, token: &str) -> String {
    let base = site_url.trim_end_matches('/');
    format!("{}/{}/{}", base, op, token)
}

/// Ensure a sync token exists for the given user+repo pair.
///
/// Only repo members may obtain a sync token (matches official seafile: the
/// token is minted from `repo-tokens`/`download-info`, both of which require
/// repo permission). Non-members get `Forbidden`.
fn build_repo_info_from_model(
    r: &repo::Model,
    owner_email: &str,
    permission: &str,
    user_id: i32,
    _extra_fields: bool,
) -> RepoInfo {
    let encrypted = r.encrypted != 0;
    let type_ = if r.owner_id == user_id {
        "repo".to_string()
    } else {
        "srepo".to_string()
    };
    RepoInfo {
        id: r.id.clone(),
        name: r.name.clone(),
        desc: r.description.clone(),
        owner: owner_email.to_string(),
        encrypted,
        enc_version: if encrypted {
            Some(r.enc_version as i32)
        } else {
            None
        },
        size: r.size,
        mtime: r.updated_at,
        permission: permission.to_string(),
        head_commit_id: r.head_commit_id.clone(),
        type_,
        virtual_: false,
        root: None,
        salt: if encrypted && r.enc_version >= 3 {
            Some(r.salt.clone())
        } else {
            None
        },
        magic: if encrypted { r.magic.clone() } else { None },
        random_key: if encrypted {
            r.random_key.clone()
        } else {
            None
        },
        repo_version: r.repo_version,
        lib_need_decrypt: if encrypted { Some(true) } else { None },
        repo_id_dup: None,
        repo_name_dup: None,
        token: None,
        email: None,
    }
}

impl RepoService {
    /// List all repos accessible to the given user (v2 API).
    pub async fn list_repos(
        repos: &Repositories,
        user_id: i32,
        email: &str,
    ) -> Result<Vec<RepoInfo>, AppError> {
        let memberships = repos.member.find_by_user_id(user_id).await?;

        // Batch-load all member repos in one query instead of one per member.
        let repo_ids: Vec<String> = memberships.iter().map(|m| m.repo_id.clone()).collect();
        let repos_map: HashMap<String, repo::Model> = repos
            .repo
            .find_by_ids(&repo_ids)
            .await?
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();

        let mut result = Vec::new();
        for m in memberships {
            if let Some(r) = repos_map.get(&m.repo_id) {
                result.push(build_repo_info_from_model(
                    r,
                    email,
                    &m.permission,
                    user_id,
                    false,
                ));
            }
        }

        Ok(result)
    }

    /// Create a new repo.
    ///
    /// Returns the created RepoInfo and the sync token value.
    pub async fn create_repo(
        db: &DatabaseConnection,
        repos: &Repositories,
        user_id: i32,
        email: &str,
        name: &str,
        desc: &str,
        repo_id_opt: Option<String>,
        encrypted_val: i32,
        enc_version_val: i32,
        magic: Option<String>,
        random_key: Option<String>,
    ) -> Result<(RepoInfo, String), AppError> {
        let name = validate_repo_name(name)?;
        let repo_id = repo_id_opt.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let now = chrono::Utc::now().timestamp();

        let params = crate::repository::repo::CreateRepoParams {
            id: repo_id.clone(),
            name: name.clone(),
            description: desc.to_string(),
            owner_id: user_id,
            encrypted: encrypted_val as i8,
            enc_version: enc_version_val as i8,
            magic: magic.clone(),
            random_key: random_key.clone(),
            salt: String::new(),
            permission: "rw".to_string(),
            created_at: now,
            updated_at: now,
        };
        repos.repo.create_repo(params).await?;

        repos
            .member
            .create_member(crate::repository::member::CreateMemberParams {
                repo_id: repo_id.clone(),
                user_id,
                permission: "rw".to_string(),
                created_at: now,
            })
            .await?;

        // Generate a sync token
        let token_value = generate_sync_token();
        repos
            .sync_token
            .create(&repo_id, user_id, token_value.clone(), None, now)
            .await?;

        let encrypted = encrypted_val == 1;

        // Log repo creation activity (best-effort)
        activity_log::log_activity(
            db, &repo_id, "create", "repo", "/", user_id, None, None, None, None, None,
        )
        .await;

        let repo_info = RepoInfo {
            id: repo_id.clone(),
            name: name.to_string(),
            desc: desc.to_string(),
            owner: email.to_string(),
            encrypted,
            enc_version: if encrypted {
                Some(enc_version_val)
            } else {
                None
            },
            size: 0,
            mtime: now,
            permission: "rw".to_string(),
            head_commit_id: None,
            type_: "repo".to_string(),
            virtual_: false,
            root: None,
            salt: None,
            magic: if encrypted { magic } else { None },
            random_key: if encrypted { random_key } else { None },
            repo_version: 1,
            lib_need_decrypt: if encrypted { Some(true) } else { None },
            repo_id_dup: Some(repo_id),
            repo_name_dup: Some(name.to_string()),
            token: Some(token_value.clone()),
            email: Some(email.to_string()),
        };

        Ok((repo_info, token_value))
    }

    /// Get a single repo's details (v2 API).
    pub async fn get_repo(
        repos: &Repositories,
        repo_id: &str,
        user_id: i32,
        email: &str,
    ) -> Result<RepoInfo, AppError> {
        let r = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        let membership = repos.member.find_by_repo_and_user(repo_id, user_id).await?;
        let permission = membership
            .map(|m| m.permission)
            .unwrap_or_else(|| "rw".to_string());

        let root = if let Some(ref cmmt_id) = r.head_commit_id {
            repos
                .commit
                .find_by_repo_and_commit_id(&r.id, cmmt_id)
                .await?
                .map(|c| c.root_id)
        } else {
            None
        };

        let encrypted = r.encrypted != 0;
        let type_ = if r.owner_id == user_id {
            "repo"
        } else {
            "srepo"
        };
        let enc_version = if encrypted {
            Some(r.enc_version as i32)
        } else {
            None
        };
        let salt = if encrypted && r.enc_version >= 3 {
            Some(r.salt.clone())
        } else {
            None
        };
        let magic = if encrypted { r.magic.clone() } else { None };
        let random_key = if encrypted {
            r.random_key.clone()
        } else {
            None
        };

        Ok(RepoInfo {
            id: r.id.clone(),
            name: r.name.clone(),
            desc: r.description.clone(),
            owner: email.to_string(),
            encrypted,
            enc_version,
            size: r.size,
            mtime: r.updated_at,
            permission,
            head_commit_id: r.head_commit_id.clone(),
            type_: type_.to_string(),
            virtual_: false,
            root,
            salt,
            magic,
            random_key,
            repo_version: r.repo_version,
            lib_need_decrypt: if encrypted { Some(true) } else { None },
            repo_id_dup: None,
            repo_name_dup: None,
            token: None,
            email: None,
        })
    }

    /// Rename a repo. Only the owner can rename.
    pub async fn rename_repo(
        db: &DatabaseConnection,
        repos: &Repositories,
        repo_id: &str,
        user_id: i32,
        new_name: &str,
    ) -> Result<(), AppError> {
        let r = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        if r.owner_id != user_id {
            return Err(AppError::Forbidden);
        }

        let new_name = validate_repo_name(new_name)?;

        let now = chrono::Utc::now().timestamp();
        repos.repo.rename_repo(repo_id, &new_name, now).await?;

        // Log repo rename activity after the update so the detail captures the
        // new name, while old_path/old_repo_name hold the previous name. The
        // Android client renders rename events as "old_name => name", so
        // old_path must be non-null to avoid a client-side crash.
        activity_log::log_activity(
            db,
            repo_id,
            "rename",
            "repo",
            "/",
            user_id,
            Some(&r.name),
            None,
            None,
            Some(&r.name),
            None,
        )
        .await;

        Ok(())
    }

    /// Update a repo's name, description, and/or history retention settings.
    /// Only the owner can update.
    pub async fn update_repo(
        db: &DatabaseConnection,
        repos: &Repositories,
        repo_id: &str,
        user_id: i32,
        new_name: Option<String>,
        new_description: Option<String>,
        history_limit: Option<i32>,
        history_ttl_days: Option<i32>,
    ) -> Result<(), AppError> {
        let r = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        if r.owner_id != user_id {
            return Err(AppError::Forbidden);
        }

        let now = chrono::Utc::now().timestamp();

        // Validate name if provided.
        let validated_name = new_name
            .as_ref()
            .map(|n| validate_repo_name(n))
            .transpose()?;

        // Validate history retention settings (must be >= 0; 0 = unlimited).
        if history_limit.is_some_and(|v| v < 0) || history_ttl_days.is_some_and(|v| v < 0) {
            return Err(AppError::BadRequest(
                "history_limit and history_ttl_days must be >= 0".into(),
            ));
        }

        // Only record a rename activity when the name actually changes.
        // Clients commonly echo the current name back when updating a repo's
        // description, which must not be logged as a rename.
        let name_changed = matches!(
            (validated_name.as_deref(), r.name.as_str()),
            (Some(new_name), old) if new_name != old
        );

        repos
            .repo
            .update_repo_details(
                repo_id,
                validated_name.as_deref(),
                new_description.as_deref(),
                history_limit,
                history_ttl_days,
                now,
            )
            .await?;

        // Log the rename activity after the update so the detail captures the
        // new name, with old_path/old_repo_name holding the previous name.
        if name_changed {
            activity_log::log_activity(
                db,
                repo_id,
                "rename",
                "repo",
                "/",
                user_id,
                Some(&r.name),
                None,
                None,
                Some(&r.name),
                None,
            )
            .await;
        }

        Ok(())
    }

    /// Delete a repo. Only the owner can delete.
    pub async fn delete_repo(
        db: &DatabaseConnection,
        repos: &Repositories,
        repo_id: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let r = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        if r.owner_id != user_id {
            return Err(AppError::Forbidden);
        }

        // Record deleted repo in trash before cascade-delete
        if let Err(e) = crate::fs::core::trash::add_deleted_repo(
            repos,
            repo_id,
            &r.name,
            r.head_commit_id.as_deref(),
            r.owner_id,
            r.size,
        )
        .await
        {
            tracing::warn!("Failed to record deleted repo in trash: {e}");
        }

        // Log repo deletion activity BEFORE deleting the repo
        activity_log::log_activity(
            db, repo_id, "delete", "repo", "/", user_id, None, None, None, None, None,
        )
        .await;

        // Cascade-delete related records
        repos.member.delete_by_repo(repo_id).await?;

        repos.sync_token.delete_by_repo(repo_id).await?;

        // Delete the repo itself
        repos.repo.delete_by_id(repo_id).await?;

        Ok(())
    }

    /// Get download info for a repo.
    pub async fn download_info(
        repos: &Repositories,
        repo_id: &str,
        user_id: i32,
    ) -> Result<DownloadInfoResponse, AppError> {
        let r = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        let u = repos
            .user
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("user not found".into()))?;

        let token_value = ensure_sync_token(repos, repo_id, user_id).await?;

        Ok(DownloadInfoResponse {
            repo_id: repo_id.to_string(),
            repo_name: r.name,
            token: token_value,
            email: u.email,
            relay_id: None,
            relay_addr: None,
            relay_port: None,
            enc_version: r.enc_version as i32,
            encrypted: if r.encrypted == 1 {
                "true".to_string()
            } else {
                "false".to_string()
            },
            magic: r.magic,
            random_key: r.random_key,
            repo_version: 1,
            salt: if r.salt.is_empty() {
                None
            } else {
                Some(r.salt.clone())
            },
            permission: r.permission,
        })
    }

    /// Get an upload link URL for the given repo.
    pub async fn get_upload_link(
        repos: &Repositories,
        token_manager: &AccessTokenManager,
        site_url: &str,
        repo_id: &str,
        user_id: i32,
        email: &str,
        parent_dir: &str,
        from: Option<&str>,
        replace: Option<&str>,
    ) -> Result<String, AppError> {
        // Verify repo exists
        repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        // Verify caller has write permission on the repo
        crate::domain::permission::check_repo_write_permission(
            repos.member.as_ref(),
            repo_id,
            user_id,
        )
        .await?;

        let token = token_manager.generate(repo_id, user_id, email, "upload", parent_dir);

        let is_web = from == Some("web");
        let op = if is_web { "upload-aj" } else { "upload-api" };
        let mut url = build_op_url(site_url, op, &token);

        if !is_web && replace == Some("1") {
            url.push_str("?replace=1");
        }

        Ok(url)
    }

    /// Get an update link URL for the given repo.
    pub async fn get_update_link(
        repos: &Repositories,
        token_manager: &AccessTokenManager,
        site_url: &str,
        repo_id: &str,
        user_id: i32,
        email: &str,
        parent_dir: &str,
        from: Option<&str>,
    ) -> Result<String, AppError> {
        // Verify repo exists
        repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        // Verify caller has write permission on the repo
        crate::domain::permission::check_repo_write_permission(
            repos.member.as_ref(),
            repo_id,
            user_id,
        )
        .await?;

        let token = token_manager.generate(repo_id, user_id, email, "update", parent_dir);

        let op = if from == Some("web") {
            "update-aj"
        } else {
            "update-api"
        };
        let url = build_op_url(site_url, op, &token);

        Ok(url)
    }

    /// Batch get sync tokens for multiple repos.
    ///
    /// Repos the caller has no permission on are silently skipped (matches
    /// official seahub's `RepoTokensView` behaviour) so desktop clients can
    /// batch-request without tripping over non-member repos.
    pub async fn repo_tokens(
        repos: &Repositories,
        repo_ids: &[&str],
        user_id: i32,
    ) -> Result<HashMap<String, String>, AppError> {
        let ids: Vec<String> = repo_ids.iter().map(|s| s.to_string()).collect();

        // Batch-load repos (existence + owner), memberships and existing sync
        // tokens so the whole batch is ~3 queries instead of 2-3 per repo.
        let repo_map: HashMap<String, repo::Model> = repos
            .repo
            .find_by_ids(&ids)
            .await?
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        let memberships = repos.member.find_by_repo_ids(&ids, user_id).await?;
        let member_repo_ids: HashSet<&str> =
            memberships.iter().map(|m| m.repo_id.as_str()).collect();
        let token_rows = repos
            .sync_token
            .find_by_repos_and_user(&ids, user_id)
            .await?;
        let token_map: HashMap<&str, &infra::entity::sync_token::Model> =
            token_rows.iter().map(|t| (t.repo_id.as_str(), t)).collect();

        let mut result = HashMap::new();
        for repo_id in repo_ids {
            // Repo deleted → skip (was NotFound).
            let Some(repo_model) = repo_map.get(*repo_id) else {
                continue;
            };
            // Owner has access even without a member row; memberships grant
            // read access. Non-members are skipped (was Forbidden).
            let has_access = repo_model.owner_id == user_id || member_repo_ids.contains(*repo_id);
            if !has_access {
                continue;
            }
            let token = match token_map.get(*repo_id) {
                Some(t) => t.token.clone(),
                None => {
                    let value = generate_sync_token();
                    let now = chrono::Utc::now().timestamp();
                    repos
                        .sync_token
                        .create(repo_id, user_id, value.clone(), None, now)
                        .await?;
                    value
                }
            };
            result.insert(repo_id.to_string(), token);
        }
        Ok(result)
    }

    /// List repos with v2.1 response format.
    pub async fn list_repos_v21(
        repos: &Repositories,
        user_id: i32,
        email: &str,
    ) -> Result<V21RepoListResponse, AppError> {
        let memberships = repos.member.find_by_user_id(user_id).await?;

        // Batch-load all member repos, then the profiles of non-owned repos'
        // owners, in two queries instead of 1-2 per repo.
        let repo_ids: Vec<String> = memberships.iter().map(|m| m.repo_id.clone()).collect();
        let repos_map: HashMap<String, repo::Model> = repos
            .repo
            .find_by_ids(&repo_ids)
            .await?
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        let owner_ids: Vec<i32> = repos_map
            .values()
            .filter(|r| r.owner_id != user_id)
            .map(|r| r.owner_id)
            .collect();
        let owners: HashMap<i32, user::Model> = repos
            .user
            .find_by_ids(&owner_ids)
            .await?
            .into_iter()
            .map(|u| (u.id, u))
            .collect();

        let mut repos_list = Vec::new();
        for m in &memberships {
            let Some(r) = repos_map.get(&m.repo_id) else {
                continue;
            };
            let is_owner = r.owner_id == user_id;
            let repo_type = if is_owner { "mine" } else { "shared" };
            let (owner_email, owner_name) = resolve_owner(r.owner_id, email, is_owner, &owners);

            repos_list.push(V21RepoInfo {
                repo_id: r.id.clone(),
                repo_name: r.name.clone(),
                repo_desc: r.description.clone(),
                permission: m.permission.clone(),
                encrypted: r.encrypted != 0,
                type_: repo_type.to_string(),
                size: r.size,
                last_modified: timestamp_rfc3339(r.updated_at),
                mtime: r.updated_at,
                owner_email,
                owner_name,
            });
        }

        Ok(V21RepoListResponse { repos: repos_list })
    }

    /// Get a single repo with v2.1 response format.
    pub async fn get_repo_v21(
        repos: &Repositories,
        repo_id: &str,
        user_id: i32,
        email: &str,
    ) -> Result<V21RepoInfo, AppError> {
        let membership = repos
            .member
            .find_by_repo_and_user(repo_id, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        let r = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        let is_owner = r.owner_id == user_id;
        let repo_type = if is_owner { "mine" } else { "shared" };
        let owners: HashMap<i32, user::Model> = if is_owner {
            HashMap::new()
        } else {
            let mut m = HashMap::new();
            if let Some(u) = repos.user.find_by_id(r.owner_id).await? {
                m.insert(u.id, u);
            }
            m
        };
        let (owner_email, owner_name) = resolve_owner(r.owner_id, email, is_owner, &owners);

        Ok(V21RepoInfo {
            repo_id: r.id,
            repo_name: r.name,
            repo_desc: r.description,
            permission: membership.permission,
            encrypted: r.encrypted != 0,
            type_: repo_type.to_string(),
            size: r.size,
            last_modified: timestamp_rfc3339(r.updated_at),
            mtime: r.updated_at,
            owner_email,
            owner_name,
        })
    }
}

/// Minimum repo data for the left panel sidebar.
#[derive(Clone)]
pub struct LeftPanelRepo {
    pub id: String,
    pub name: String,
    pub size_display: String,
}

/// Query all repos for the given user, returning left-panel data.
pub async fn load_left_panel_repos(
    repos: &Repositories,
    user_id: i32,
) -> Result<Vec<LeftPanelRepo>, AppError> {
    let members = repos.member.find_by_user_id(user_id).await?;

    // Batch-load member repos in one query instead of one per member.
    let repo_ids: Vec<String> = members.iter().map(|m| m.repo_id.clone()).collect();
    let repos_map: HashMap<String, repo::Model> = repos
        .repo
        .find_by_ids(&repo_ids)
        .await?
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect();

    let mut repo_list = Vec::with_capacity(members.len());
    for m in members {
        if let Some(r) = repos_map.get(&m.repo_id) {
            repo_list.push(LeftPanelRepo {
                id: r.id.clone(),
                name: r.name.clone(),
                size_display: format_size(r.size),
            });
        }
    }
    Ok(repo_list)
}

/// Resolve a repo owner's email + display name, using the requester's email
/// when the requester owns the repo.
///
/// `owners` is a pre-fetched id → user map; callers batch-load it once.
fn resolve_owner(
    owner_id: i32,
    requester_email: &str,
    is_owner: bool,
    owners: &HashMap<i32, user::Model>,
) -> (String, String) {
    if is_owner {
        (
            requester_email.to_string(),
            requester_email.split('@').next().unwrap_or("").to_string(),
        )
    } else {
        match owners.get(&owner_id) {
            Some(u) => (u.email.clone(), u.nickname()),
            None => (String::new(), String::new()),
        }
    }
}

/// Validate a repo name before it is persisted.
///
/// Repo names are rendered into inline `<script>` blocks (`|json|safe` in the
/// toolbar template), so `<`, `>`, `&` and `"` are rejected outright — along
/// with control characters, path separators and over-long names — to prevent
/// script breakout.
fn validate_repo_name(name: &str) -> Result<String, AppError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.len() > 255
        || trimmed.contains('/')
        || trimmed.contains(['<', '>', '&', '"'])
        || trimmed.chars().any(char::is_control)
    {
        return Err(AppError::BadRequest("invalid repo name".into()));
    }
    Ok(trimmed.to_string())
}
