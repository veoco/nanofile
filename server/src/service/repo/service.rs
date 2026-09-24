use sea_orm::DatabaseConnection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::AccessTokenManager;
use crate::domain::device::PeerStamp;
use crate::repository::Repositories;
use crate::service::auth::token::{ensure_sync_token_for, ensure_sync_tokens_for_repos};
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
    pub owner_name: String,
    /// nanofile does not model group-owned repos; always null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groupid: Option<i32>,
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
    /// `pwd_hash` verifier of a Seafile 11+ library. Present only when the
    /// library uses one, in which case `magic` is **absent**.
    ///
    /// These three fields are a superset of seahub's `GET /api2/repos/{id}/`
    /// (which returns only `magic`/`random_key`/`salt`), but they are required
    /// here: nanofile's `POST /api2/repos/` answers with this shape, and the
    /// desktop client parses the create response with the same
    /// `RepoDownloadInfo::fromDict()` it uses for `download-info` — it then
    /// feeds `pwd_hash*` into the follow-up `cloneRepo()`, which fails with
    /// "Bad magic" if the algorithm is missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwd_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwd_hash_algo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwd_hash_params: Option<String>,
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
    /// seahub's `repo_download_info()` sends `1` for an encrypted library and
    /// the **empty string** otherwise, and the desktop client reads this field
    /// with `QVariant::toInt()` (`requests.cpp:173`). A JSON string `"true"`
    /// would therefore read back as `0` and every encrypted library would look
    /// unencrypted at clone time, so the value must stay numeric-ish.
    pub encrypted: serde_json::Value,
    pub magic: Option<String>,
    pub random_key: Option<String>,
    pub repo_version: i32,
    pub salt: Option<String>,
    /// `pwd_hash` verifier triple (see [`RepoInfo`]). seahub returns these from
    /// `repo_download_info()` even when they are empty; the desktop client
    /// copies the non-empty ones into the clone task's `more_info`, which is the
    /// only place `clone-mgr.c` learns the algorithm from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwd_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwd_hash_algo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwd_hash_params: Option<String>,
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
    pub owner_contact_email: String,
    /// nanofile does not model group-owned repos; always null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
}

// ── Service ─────────────────────────────────────────────────────────────

pub struct RepoService;

/// The `pwd_hash` verifier triple of a Seafile 11+ encrypted library,
/// mirroring seafile-server's `RepoCryptInfo` (`server/repo-mgr.c`).
///
/// All three values travel together: the algorithm is what tells a client which
/// KDF to run, and the parameters what to run it with. `params = None` means
/// "the algorithm's defaults" (upstream stores an SQL NULL for that, and so do
/// we; the commit then omits the key, which reads back as NULL on the client and
/// lands on the same defaults).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PwdHash {
    pub hash: Option<String>,
    pub algo: Option<String>,
    pub params: Option<String>,
}

impl PwdHash {
    /// Build one from the three wire fields.
    pub fn from_request(
        hash: Option<String>,
        algo: Option<String>,
        params: Option<String>,
    ) -> Self {
        Self { hash, algo, params }
    }

    /// Treat blank fields as absent (the clients send `""` for anything they do
    /// not set, and seahub's own `request.data.get(...)` yields `None`), and
    /// drop a triple that has no algorithm.
    pub fn normalized(self) -> Self {
        fn clean(v: Option<String>) -> Option<String> {
            v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        }
        let mut out = Self {
            hash: clean(self.hash),
            algo: clean(self.algo),
            params: clean(self.params),
        };
        if out.algo.is_none() {
            // Mirrors `repo_crypt_info_new` + `create_repo_common`: nothing is
            // read from `crypt_info` unless `pwd_hash_algo` is set, so a client
            // that sends only `pwd_hash` gets it ignored rather than having an
            // unusable verifier stored (which would leave the library
            // unopenable).
            out.hash = None;
            out.params = None;
        }
        out
    }

    /// Whether the request carries a usable verifier triple.
    ///
    /// Upstream gates *every* `pwd_hash` field on the algorithm
    /// (`if (crypt_info && crypt_info->pwd_hash_algo)`), so a lone `pwd_hash`
    /// without an algorithm is not a verifier at all.
    pub fn is_present(&self) -> bool {
        self.algo.is_some()
    }

    /// The three `repos` columns.
    pub fn to_columns(&self) -> (Option<String>, Option<String>, Option<String>) {
        (self.hash.clone(), self.algo.clone(), self.params.clone())
    }

    /// seafile-server's `create_repo_common()` validation for the triple.
    ///
    /// * an algorithm outside `{pbkdf2_sha256, argon2id}` is rejected
    ///   ("Unsupported encryption algothrims")
    /// * the hash must be exactly 64 characters ("Bad pwd_hash") — we also
    ///   require ASCII hex, because a 64-character string that is not hex can
    ///   never equal `hex(derived_key)` and would leave the library silently
    ///   unopenable
    /// * the parameters are parsed and bounded (see
    ///   [`infra::crypto::pwd_hash::validate_params`])
    pub fn validate(&self) -> Result<(), AppError> {
        let algo = self
            .algo
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("pwd_hash_algo required with pwd_hash".into()))?;
        if !infra::crypto::pwd_hash::is_supported_algo(algo) {
            return Err(AppError::BadRequest(format!(
                "unsupported pwd_hash algorithm '{algo}'; use '{}' or '{}'",
                infra::crypto::pwd_hash::ALGO_PBKDF2_SHA256,
                infra::crypto::pwd_hash::ALGO_ARGON2ID,
            )));
        }
        match self.hash.as_deref() {
            Some(h) if h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()) => {}
            _ => {
                return Err(AppError::BadRequest(
                    "pwd_hash must be 64 hex characters".into(),
                ));
            }
        }
        infra::crypto::pwd_hash::validate_params(algo, self.params.as_deref())
            .map_err(AppError::BadRequest)?;
        Ok(())
    }
}

/// The server-side encrypted-library policy, i.e. seahub's
/// `ENCRYPTED_LIBRARY_VERSION` / `ENCRYPTED_LIBRARY_PWD_HASH_ALGO` /
/// `ENCRYPTED_LIBRARY_PWD_HASH_PARAMS`.
///
/// Used when a client asks for an encrypted library by sending only a password
/// (Android, the web UI): the *server* generates the verifier.
#[derive(Debug, Clone)]
pub struct EncryptedLibraryPolicy {
    pub enc_version: i32,
    /// `None` keeps the legacy `magic` flow, which is upstream's default
    /// (`ENCRYPTED_LIBRARY_PWD_HASH_ALGO = ""`).
    pub pwd_hash_algo: Option<String>,
    pub pwd_hash_params: Option<String>,
}

impl EncryptedLibraryPolicy {
    /// Read the policy from the server config, treating empty strings as
    /// "unset" (seahub passes `ENCRYPTED_LIBRARY_PWD_HASH_ALGO or None`).
    pub fn from_config(config: &infra::config::ServerConfig) -> Self {
        fn clean(v: &Option<String>) -> Option<String> {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        }
        Self {
            enc_version: config.encrypted_library_version,
            pwd_hash_algo: clean(&config.encrypted_library_pwd_hash_algo),
            pwd_hash_params: clean(&config.encrypted_library_pwd_hash_params),
        }
    }
}

/// The encryption block of a library, as every read path needs it.
///
/// Centralises the "a `pwd_hash` library has no `magic`" rule: upstream writes
/// the `magic` commit field only when `!commit->pwd_hash`, and a client that saw
/// both would verify the password against a `magic` the server never stored.
#[derive(Debug, Clone, Default)]
pub struct RepoCrypto {
    pub encrypted: bool,
    pub enc_version: Option<i32>,
    pub magic: Option<String>,
    pub key: Option<String>,
    pub salt: Option<String>,
    pub pwd_hash: Option<String>,
    pub pwd_hash_algo: Option<String>,
    pub pwd_hash_params: Option<String>,
}

impl RepoCrypto {
    pub fn from_model(r: &repo::Model) -> Self {
        let encrypted = r.encrypted != 0;
        if !encrypted {
            return Self::default();
        }
        let pwd_hash = r.pwd_hash.clone();
        Self {
            encrypted: true,
            enc_version: Some(r.enc_version as i32),
            // Suppressed for a `pwd_hash` library: upstream never emits `magic`
            // alongside `pwd_hash`, and `commit_from_json_object()` substitutes
            // `magic = pwd_hash` when the field is missing.
            magic: if pwd_hash.is_some() {
                None
            } else {
                r.magic.clone()
            },
            key: r.random_key.clone(),
            salt: if r.enc_version >= 3 && !r.salt.is_empty() {
                Some(r.salt.clone())
            } else {
                None
            },
            pwd_hash,
            pwd_hash_algo: r.pwd_hash_algo.clone(),
            pwd_hash_params: r.pwd_hash_params.clone(),
        }
    }
}

fn build_op_url(site_url: &str, op: &str, token: &str) -> String {
    let base = site_url.trim_end_matches('/');
    format!("{}/{}/{}", base, op, token)
}

/// Build the `RepoInfo` wire shape of one library the caller can access, for a
/// list response.
///
/// `user_id` decides `type` — `repo` for the owner, `srepo` for a member — and
/// `permission` is that caller's membership row, passed in because the caller
/// has already loaded the memberships. Encryption fields are echoed from the
/// stored row, so a library read here cannot disagree with a later `get`.
///
/// The per-caller extras (`token`, `email` and the duplicated `repo_id` /
/// `repo_name`) stay empty: only the endpoints that hand the caller a credential
/// of its own fill them (see [`RepoService::create_repo`]).
fn build_repo_info_from_model(
    r: &repo::Model,
    owner_email: &str,
    owner_name: &str,
    permission: &str,
    user_id: i32,
) -> RepoInfo {
    let encrypted = r.encrypted != 0;
    let crypto = RepoCrypto::from_model(r);
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
        owner_name: owner_name.to_string(),
        groupid: None,
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
        magic: crypto.magic.clone(),
        random_key: crypto.key.clone(),
        pwd_hash: crypto.pwd_hash.clone(),
        pwd_hash_algo: crypto.pwd_hash_algo.clone(),
        pwd_hash_params: crypto.pwd_hash_params.clone(),
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

        // Batch-load the owners of non-owned repos so `owner`/`owner_name` can
        // be resolved to the real owner instead of the requester.
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

        let mut result = Vec::new();
        for m in memberships {
            let Some(r) = repos_map.get(&m.repo_id) else {
                continue;
            };
            let is_owner = r.owner_id == user_id;
            let (owner_email, owner_name) = resolve_owner(r.owner_id, email, is_owner, &owners);
            result.push(build_repo_info_from_model(
                r,
                &owner_email,
                &owner_name,
                &m.permission,
                user_id,
            ));
        }

        Ok(result)
    }

    /// Create a new repo.
    ///
    /// `peer` is the device the request came from, when it reported one: the new
    /// repository's token is issued to it and returned in
    /// [`RepoInfo::token`], so it shows up under that device immediately instead
    /// of waiting for the first sync.
    ///
    /// A caller that reported **no** device (a browser session, a unified API
    /// key) gets no token here: no client is holding one yet, so minting it would
    /// only leave the account a credential that can never be used or named.
    /// Creating a library is not asking to sync it — the sync protocol mints the
    /// shared unattributed token on the first request that actually needs one
    /// (`repo-tokens`, `download-info`, `accessible-repos`).
    ///
    /// `passwd` is the bare-password creation flow (seahub passes it straight to
    /// `seafile_api.create_repo`): with no client-supplied `magic`/`random_key`
    /// the server generates the encryption material at
    /// `policy.enc_version`. See the body for why ignoring it is not an option.
    ///
    /// `pwd_hash` is the verifier triple a client pre-computed (the desktop
    /// client, when `/api2/server-info/` advertises an algorithm); `policy` is
    /// what the **server** uses when it has to generate the verifier itself.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_repo(
        db: &DatabaseConnection,
        repos: &Repositories,
        user_id: i32,
        email: &str,
        name: &str,
        desc: &str,
        repo_id_opt: Option<String>,
        encrypted_val: i32,
        mut enc_version_val: i32,
        mut magic: Option<String>,
        mut random_key: Option<String>,
        mut salt: Option<String>,
        pwd_hash: PwdHash,
        passwd: Option<&str>,
        policy: &EncryptedLibraryPolicy,
        peer: Option<&PeerStamp>,
        sync_token_ttl_days: u64,
    ) -> Result<RepoInfo, AppError> {
        let name = validate_repo_name(name)?;
        // The client may propose a repo id (the sync clients generate their
        // own). Only accept a well-formed UUID: the id is part of DNS-free
        // URL paths, is interpolated into on-disk directory names (temp
        // uploads, thumbnail cache) and is compared against sync tokens, so a
        // free-form string such as "../../x" or "/etc/cron.d/y" would escape
        // the storage root. Anything else is rejected rather than
        // silently replaced, so a client that expected its own id learns that
        // it was not used.
        let repo_id = match repo_id_opt {
            Some(client_id) => {
                let parsed = uuid::Uuid::parse_str(client_id.trim())
                    .map_err(|_| AppError::BadRequest("repo_id must be a valid UUID".into()))?;
                parsed.to_string()
            }
            None => uuid::Uuid::new_v4().to_string(),
        };
        let now = chrono::Utc::now().timestamp();

        let mut pwd_hash = pwd_hash.normalized();

        // seahub's `seafile_api.create_repo(name, desc, user, passwd,
        // enc_version=ENCRYPTED_LIBRARY_VERSION, ...)`: when a client asks for
        // an encrypted library by sending only a password, the **server**
        // generates the encryption material. The Android client does exactly
        // this (`NewRepoViewModel.createNewRepo` posts `name`/`desc`/`passwd`
        // and then reads `magic`/`random_key`/`salt` back from
        // `GET /api2/repos/{id}/`), as does the desktop client against a
        // pre-4.4 server. Treating the password as noise would create a plain
        // library and silently drop the password the user typed.
        let client_supplied_keys = magic.as_deref().is_some_and(|m| !m.trim().is_empty())
            || random_key.as_deref().is_some_and(|k| !k.trim().is_empty())
            || pwd_hash.is_present();
        let passwd = passwd.filter(|p| !p.is_empty());
        if let Some(pwd) = passwd
            && !client_supplied_keys
        {
            enc_version_val = policy.enc_version;
            if !matches!(enc_version_val, 2 | 4) {
                return Err(AppError::BadRequest(
                    "server.encrypted_library_version must be 2 or 4 to create a library \
                     from a bare password"
                        .into(),
                ));
            }
            // enc_version >= 3 uses a per-library random salt; the client reads
            // it back from the repo object and needs it in every commit.
            let repo_salt = if enc_version_val >= 3 {
                infra::crypto::key_derivation::generate_repo_salt()
            } else {
                String::new()
            };
            if let Some(algo) = policy.pwd_hash_algo.as_deref() {
                // Seafile 11+: a configured algorithm replaces `magic`
                // (`seaf_repo_manager_create_new_repo` calls
                // `seafile_generate_pwd_hash` *instead of*
                // `seafile_generate_magic`). Seadroid and the web UI then need
                // the server to be able to verify the password it just hashed.
                infra::crypto::pwd_hash::validate_params(algo, policy.pwd_hash_params.as_deref())
                    .map_err(AppError::BadRequest)?;
                let hash = infra::crypto::pwd_hash::derive_pwd_hash(
                    &repo_id,
                    pwd,
                    enc_version_val,
                    &repo_salt,
                    algo,
                    policy.pwd_hash_params.as_deref(),
                )
                .map_err(|e| AppError::BadRequest(format!("pwd_hash generation failed: {e}")))?;
                magic = None;
                pwd_hash = PwdHash {
                    hash: Some(hash),
                    algo: Some(algo.to_string()),
                    params: policy.pwd_hash_params.clone(),
                };
            } else {
                magic = Some(
                    infra::crypto::key_derivation::generate_magic(
                        &repo_id,
                        pwd,
                        enc_version_val,
                        &repo_salt,
                    )
                    .map_err(|e| AppError::BadRequest(format!("magic generation failed: {e}")))?,
                );
            }
            random_key = Some(
                infra::crypto::key_derivation::generate_random_key_for_repo(
                    pwd,
                    enc_version_val,
                    &repo_salt,
                )
                .map_err(|e| AppError::BadRequest(format!("random_key generation failed: {e}")))?,
            );
            salt = if repo_salt.is_empty() {
                None
            } else {
                Some(repo_salt)
            };
        }

        // The desktop client sends `enc_version`/`magic`/`random_key` instead
        // of the legacy `encrypted` flag, so a library carrying encryption
        // material is encrypted even when the flag is absent (it used to be
        // created as a plain library, making its files unreadable).
        let has_enc_material = magic.as_deref().is_some_and(|m| !m.trim().is_empty())
            || random_key.as_deref().is_some_and(|k| !k.trim().is_empty())
            || pwd_hash.is_present();
        let encrypted_val = if encrypted_val != 0 || has_enc_material || enc_version_val >= 2 {
            1
        } else {
            0
        };

        if encrypted_val == 1 {
            // The server can only operate encrypted repos of version 2/4 (v1/v3
            // are rejected by derive_key), and magic/random_key come from the
            // client — reject a repo whose version/keys the server could never use.
            if !matches!(enc_version_val, 2 | 4) {
                return Err(AppError::BadRequest(
                    "unsupported enc_version (only 2 and 4 are supported)".into(),
                ));
            }
            if pwd_hash.is_present() {
                // The client computed the verifier itself. Validate it the way
                // `create_repo_common()` does and drop any `magic` it also sent:
                // upstream writes the commit's `magic` field only when
                // `!pwd_hash`, and a client that saw both would verify the
                // password against a value this server never stored.
                pwd_hash.validate()?;
                magic = None;
            } else {
                let magic_ok = magic
                    .as_deref()
                    .is_some_and(|m| m.len() == 64 && m.chars().all(|c| c.is_ascii_hexdigit()));
                if !magic_ok {
                    return Err(AppError::BadRequest("magic must be 64 hex chars".into()));
                }
            }
            let random_key_ok = random_key
                .as_deref()
                .is_some_and(|k| k.len() == 96 && k.chars().all(|c| c.is_ascii_hexdigit()));
            if !random_key_ok {
                return Err(AppError::BadRequest(
                    "random_key must be 96 hex chars".into(),
                ));
            }
            // enc_version 4 uses a per-library random salt that the client
            // generated; losing it would make the library undecryptable (the
            // client derives `magic`/`random_key`/`pwd_hash` from it), so
            // require and store it. (nanofile used to accept v4 and silently
            // store an empty salt, producing a library the client could not
            // open.)
            if enc_version_val == 4 && !salt.as_deref().is_some_and(|s| !s.trim().is_empty()) {
                return Err(AppError::BadRequest(
                    "salt is required for enc_version 4".into(),
                ));
            }
        }

        let (pwd_hash_col, pwd_hash_algo_col, pwd_hash_params_col) = pwd_hash.to_columns();
        let params = crate::repository::repo::CreateRepoParams {
            id: repo_id.clone(),
            name: name.clone(),
            description: desc.to_string(),
            owner_id: user_id,
            encrypted: encrypted_val as i8,
            enc_version: enc_version_val as i8,
            magic: magic.clone(),
            random_key: random_key.clone(),
            // v2 libraries use a fixed salt (empty here); v4 stores the
            // client-generated (or server-generated) per-library salt.
            salt: if enc_version_val == 4 {
                salt.clone().unwrap_or_default()
            } else {
                String::new()
            },
            pwd_hash: pwd_hash_col,
            pwd_hash_algo: pwd_hash_algo_col,
            pwd_hash_params: pwd_hash_params_col,
            permission: "rw".to_string(),
            created_at: now,
            updated_at: now,
            r#type: "repo".to_string(),
        };
        let created = repos.repo.create_repo(params).await?;

        repos
            .member
            .create_member(crate::repository::member::CreateMemberParams {
                repo_id: repo_id.clone(),
                user_id,
                permission: "rw".to_string(),
                created_at: now,
            })
            .await?;

        // Issue the repository's sync token, attributed to the creating device
        // when it reported one. The membership was just created, so the
        // permission re-check inside the resolver would only repeat it.
        //
        // A device-less creator (a browser session, a unified API key) gets no
        // token: see the note on this function. The unattributed token is minted
        // on demand instead, by the first sync request that needs one.
        let token_value = match peer {
            Some(peer) => Some(
                ensure_sync_tokens_for_repos(
                    repos,
                    std::slice::from_ref(&repo_id),
                    user_id,
                    Some(peer),
                    sync_token_ttl_days,
                )
                .await?
                .remove(&repo_id)
                .ok_or_else(|| AppError::internal("sync token was not resolved"))?,
            ),
            None => None,
        };

        let encrypted = encrypted_val == 1;
        // Everything encryption-related is echoed from the row that was just
        // written, so the create response can never disagree with what later
        // reads (and therefore the clients) will see.
        let crypto = RepoCrypto::from_model(&created);

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
            owner_name: email.split('@').next().unwrap_or("").to_string(),
            groupid: None,
            encrypted,
            enc_version: crypto.enc_version,
            size: 0,
            mtime: now,
            permission: "rw".to_string(),
            head_commit_id: None,
            type_: "repo".to_string(),
            virtual_: false,
            root: None,
            // Echo the stored salt so a client that created a v4 library sees
            // the value its keys were derived from (mirrors the list/get
            // responses, which include `salt` for `enc_version >= 3`).
            salt: crypto.salt.clone(),
            magic: crypto.magic.clone(),
            random_key: crypto.key.clone(),
            pwd_hash: crypto.pwd_hash.clone(),
            pwd_hash_algo: crypto.pwd_hash_algo.clone(),
            pwd_hash_params: crypto.pwd_hash_params.clone(),
            repo_version: 1,
            lib_need_decrypt: if encrypted { Some(true) } else { None },
            repo_id_dup: Some(repo_id),
            repo_name_dup: Some(name.to_string()),
            token: token_value,
            email: Some(email.to_string()),
        };

        Ok(repo_info)
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

        let membership = repos
            .member
            .find_by_repo_and_user(repo_id, user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        let permission = membership.permission;

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
        let crypto = RepoCrypto::from_model(&r);
        let type_ = if r.owner_id == user_id {
            "repo"
        } else {
            "srepo"
        };
        let enc_version = crypto.enc_version;
        let salt = if encrypted && r.enc_version >= 3 {
            Some(r.salt.clone())
        } else {
            None
        };
        let magic = crypto.magic.clone();
        let random_key = crypto.key.clone();

        let owner_name = if r.owner_id == user_id {
            email.split('@').next().unwrap_or("").to_string()
        } else {
            repos
                .user
                .find_by_id(r.owner_id)
                .await?
                .map(|u| u.nickname())
                .unwrap_or_default()
        };

        Ok(RepoInfo {
            id: r.id.clone(),
            name: r.name.clone(),
            desc: r.description.clone(),
            owner: email.to_string(),
            owner_name,
            groupid: None,
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
            pwd_hash: crypto.pwd_hash.clone(),
            pwd_hash_algo: crypto.pwd_hash_algo.clone(),
            pwd_hash_params: crypto.pwd_hash_params.clone(),
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
    ///
    /// The library moves to the trash: its commit graph and FS objects are
    /// archived so that restoring it brings the files back, while its blocks stay
    /// on disk (garbage collection keeps the block directory of a library that is
    /// still listed in the trash). Both the trash entry and the archive are
    /// load-bearing, so a failure to write either one aborts the delete instead
    /// of leaving a half-deleted library behind.
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

        // Record deleted repo in trash before cascade-delete: the trash entry is
        // what makes the archive reachable, and what keeps GC from reclaiming the
        // library's blocks.
        crate::fs::core::trash::add_deleted_repo(
            repos,
            repo_id,
            &r.name,
            r.head_commit_id.as_deref(),
            r.owner_id,
            r.size,
        )
        .await?;

        // Log repo deletion activity BEFORE deleting the repo
        activity_log::log_activity(
            db, repo_id, "delete", "repo", "/", user_id, None, None, None, None, None,
        )
        .await;

        // Archive the library's content and delete its rows in one transaction.
        crate::fs::core::repo_archive::archive_and_delete(db, repo_id).await?;

        // The token rows were deleted with the library; this drops the cached
        // copies so a revoked client cannot keep authenticating from the cache.
        repos.sync_token.delete_by_repo(repo_id).await?;

        // API-key bindings cascade with the repository; a key that was only
        // bound to this library is left inert, so remove it rather than leave a
        // credential that can never authenticate again.
        repos.api_key.delete_bindings_by_repo(repo_id).await?;

        Ok(())
    }

    /// Get download info for a repo.
    ///
    /// `peer` is the requesting device, when it reported one: the token handed
    /// back is that device's own, and is attributed to it at issuance.
    pub async fn download_info(
        repos: &Repositories,
        repo_id: &str,
        user_id: i32,
        peer: Option<&PeerStamp>,
        sync_token_ttl_days: u64,
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

        let token_value =
            ensure_sync_token_for(repos, repo_id, user_id, peer, sync_token_ttl_days).await?;

        // A `pwd_hash` library must not advertise a `magic`: the desktop client
        // copies the non-empty fields of this response into the clone task's
        // `more_info`, and `clone-mgr.c` verifies against `pwd_hash` when (and
        // only when) an algorithm is present.
        let crypto = RepoCrypto::from_model(&r);

        Ok(DownloadInfoResponse {
            repo_id: repo_id.to_string(),
            repo_name: r.name,
            token: token_value,
            email: u.email,
            relay_id: None,
            relay_addr: None,
            relay_port: None,
            enc_version: r.enc_version as i32,
            // seahub: `enc = 1 if repo.encrypted else ''`.
            encrypted: if r.encrypted == 1 {
                serde_json::json!(1)
            } else {
                serde_json::json!("")
            },
            magic: crypto.magic,
            random_key: crypto.key,
            repo_version: 1,
            salt: if r.salt.is_empty() {
                None
            } else {
                Some(r.salt.clone())
            },
            pwd_hash: crypto.pwd_hash,
            pwd_hash_algo: crypto.pwd_hash_algo,
            pwd_hash_params: crypto.pwd_hash_params,
            permission: r.permission,
        })
    }

    /// Get an upload link URL for the given repo.
    ///
    /// `base_url` is the caller-resolved external URL base (Host-aware when
    /// `site_url` is still the built-in default; see
    /// [`ServerConfig::download_url_base`]).
    pub async fn get_upload_link(
        repos: &Repositories,
        token_manager: &AccessTokenManager,
        base_url: &str,
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
        let mut url = build_op_url(base_url, op, &token);

        if !is_web && replace == Some("1") {
            url.push_str("?replace=1");
        }

        Ok(url)
    }

    /// Get an update link URL for the given repo.
    ///
    /// `base_url` is the caller-resolved external URL base (Host-aware when
    /// `site_url` is still the built-in default).
    pub async fn get_update_link(
        repos: &Repositories,
        token_manager: &AccessTokenManager,
        base_url: &str,
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
        let url = build_op_url(base_url, op, &token);

        Ok(url)
    }

    /// Batch get sync tokens for multiple repos.
    ///
    /// Repos the caller has no permission on are silently skipped (matches
    /// official seahub's `RepoTokensView` behaviour) so desktop clients can
    /// batch-request without tripping over non-member repos.
    ///
    /// `peer` is the requesting device: each repository gets *that device's*
    /// token, minted with the device already recorded. A caller with no device
    /// identity shares the single unattributed token per repository.
    pub async fn repo_tokens(
        repos: &Repositories,
        repo_ids: &[&str],
        user_id: i32,
        peer: Option<&PeerStamp>,
        sync_token_ttl_days: u64,
    ) -> Result<HashMap<String, String>, AppError> {
        let ids: Vec<String> = repo_ids.iter().map(|s| s.to_string()).collect();

        // Batch-load repos (existence + owner) and memberships so the whole
        // batch is ~2 queries instead of 1-2 per repo.
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

        // Only repos the caller may read are worth a token row.
        let allowed: Vec<String> = repo_ids
            .iter()
            .copied()
            .filter(|repo_id| {
                // Repo deleted → skip (was NotFound).
                let Some(repo_model) = repo_map.get(*repo_id) else {
                    return false;
                };
                // Owner has access even without a member row; memberships grant
                // read access. Non-members are skipped (was Forbidden).
                repo_model.owner_id == user_id || member_repo_ids.contains(*repo_id)
            })
            .map(str::to_string)
            .collect();

        ensure_sync_tokens_for_repos(repos, &allowed, user_id, peer, sync_token_ttl_days).await
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
                owner_contact_email: owner_email.clone(),
                owner_email,
                owner_name,
                group_id: None,
                group_name: None,
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
            owner_contact_email: owner_email.clone(),
            owner_email,
            owner_name,
            group_id: None,
            group_name: None,
        })
    }

    /// The user's default (primary) library.
    ///
    /// nanofile does not persist a per-user "default repo" flag (seahub tracks
    /// this via `UserOptions`), so the earliest owned repo is used as a stable
    /// proxy for the default library the desktop virtual-drive flow expects.
    pub async fn default_repo_id(
        repos: &Repositories,
        user_id: i32,
    ) -> Result<Option<String>, AppError> {
        Ok(repos
            .repo
            .find_earliest_by_owner(user_id)
            .await?
            .map(|r| r.id))
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
