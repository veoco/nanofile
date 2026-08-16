use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::domain::permission::{check_repo_read_permission, check_repo_write_permission};
use crate::repository::Repositories;
use crate::service::fs::file::FileService;
use base::error::AppError;
use infra::storage::DynBlockStorage;

/// Wiki config file lives in the hidden `_Internal` dir (Seafile wiki2).
pub const WIKI_CONFIG_PATH: &str = "_Internal/Wiki";
pub const WIKI_CONFIG_FILE: &str = "index.json";
/// Wiki page files live under this top-level dir in the wiki repo.
pub const WIKI_PAGES_DIR: &str = "/wiki-pages";

/// Storage layout inside a wiki repo (mirrors seahub's wiki2):
/// - `_Internal/Wiki/index.json` holds `{version, navigation, pages}`.
/// - `/wiki-pages/{doc_uuid}/{name}.md` holds the page content (markdown).
pub struct WikiService {
    repos: Arc<Repositories>,
    db: Arc<DatabaseConnection>,
    block_store: DynBlockStorage,
    file_service: FileService,
}

impl WikiService {
    pub fn new(
        repos: Arc<Repositories>,
        db: Arc<DatabaseConnection>,
        block_store: DynBlockStorage,
        file_service: FileService,
    ) -> Self {
        Self {
            repos,
            db,
            block_store,
            file_service,
        }
    }

    fn db(&self) -> &DatabaseConnection {
        self.db.as_ref()
    }

    /// Create a wiki: a new library marked `type='wiki'`, initialised with a
    /// home page and an empty navigation tree. Returns the wiki JSON as the
    /// seahub `POST /api/v2.1/wikis2/` response.
    pub async fn create_wiki(
        &self,
        user_id: i32,
        email: &str,
        name: &str,
    ) -> Result<serde_json::Value, AppError> {
        let name = validate_wiki_name(name)?;
        let repo_id = uuid::Uuid::new_v4().to_string();

        crate::service::repo::service::RepoService::create_repo(
            self.db(),
            &self.repos,
            user_id,
            email,
            &name,
            "",
            Some(repo_id.clone()),
            0,
            0,
            None,
            None,
            "wiki",
        )
        .await?;

        // Create the internal directory structure.
        self.create_dir(user_id, email, &repo_id, "/_Internal")
            .await?;
        self.create_dir(user_id, email, &repo_id, "/_Internal/Wiki")
            .await?;

        let doc_uuid = uuid::Uuid::new_v4().to_string();
        let page_id = gen_page_id();
        self.create_dir(user_id, email, &repo_id, WIKI_PAGES_DIR)
            .await?;
        let page_dir = format!("{WIKI_PAGES_DIR}/{doc_uuid}");
        self.create_dir(user_id, email, &repo_id, &page_dir).await?;

        // Initial navigation tree with a single home page.
        let config = serde_json::json!({
            "version": 1,
            "navigation": [{ "id": page_id, "type": "page" }],
            "pages": [{
                "id": page_id,
                "name": "home",
                "path": format!("{page_dir}/home.md"),
                "icon": "",
                "docUuid": doc_uuid,
                "locked": false,
            }],
        });
        self.save_wiki_config(user_id, email, &repo_id, &config)
            .await?;

        // Write the home page content.
        self.upload_markdown(
            user_id,
            email,
            &repo_id,
            &page_dir,
            "home.md",
            format!("# {}\n", name).as_bytes(),
        )
        .await?;

        Ok(wiki2_json(&repo_id, &name, email, "rw", "mine", false))
    }

    /// List all wikis accessible to the user (Seafile wiki2 response shape:
    /// `{"wikis": [...], "group_wikis": [...]}`). Group wikis are not
    /// supported yet, so `group_wikis` is always empty.
    pub async fn list_wikis_v2(
        &self,
        user_id: i32,
        email: &str,
    ) -> Result<serde_json::Value, AppError> {
        let owned = self.repos.repo.find_wiki_by_owner_id(user_id).await?;
        let all_wiki = self.repos.repo.find_all_wiki().await?;

        // Owner rows first (type=mine), then shared wiki repos the user is a
        // member of (type=shared).
        let owned_ids: std::collections::HashSet<String> =
            owned.iter().map(|r| r.id.clone()).collect();
        let memberships = self.repos.member.find_by_user_id(user_id).await?;
        let member_perms: std::collections::HashMap<String, String> = memberships
            .into_iter()
            .map(|m| (m.repo_id.clone(), m.permission.clone()))
            .collect();

        let mut items: Vec<(&infra::entity::repo::Model, &str, &str)> =
            owned.iter().map(|r| (r, "mine", "rw")).collect();
        for r in &all_wiki {
            if !owned_ids.contains(&r.id)
                && let Some(perm) = member_perms.get(&r.id)
            {
                items.push((r, "shared", perm));
            }
        }

        // Batch-load publishes and non-owner users so the list is O(1) queries
        // instead of one per wiki.
        let item_repo_ids: Vec<String> = items.iter().map(|(r, _, _)| r.id.clone()).collect();
        let publishes = self
            .repos
            .wiki2_publish
            .find_by_repo_ids(&item_repo_ids)
            .await?;
        let publish_map: std::collections::HashMap<String, &infra::entity::wiki2_publish::Model> =
            publishes.iter().map(|p| (p.repo_id.clone(), p)).collect();
        let owner_ids: Vec<i32> = items
            .iter()
            .filter(|(r, _, _)| r.owner_id != user_id)
            .map(|(r, _, _)| r.owner_id)
            .collect();
        let owners = self.repos.user.find_by_ids(&owner_ids).await?;
        let owner_map: std::collections::HashMap<i32, &infra::entity::user::Model> =
            owners.iter().map(|u| (u.id, u)).collect();

        let wikis: Vec<serde_json::Value> = items
            .into_iter()
            .map(|(r, type_, perm)| {
                let (owner_email, owner_nickname) = if r.owner_id == user_id {
                    (email.to_string(), requester_email_nickname(email))
                } else {
                    owner_map
                        .get(&r.owner_id)
                        .map(|u| (u.email.clone(), u.nickname()))
                        .unwrap_or_default()
                };
                build_wiki_item(
                    r,
                    &owner_email,
                    &owner_nickname,
                    type_,
                    perm,
                    publish_map.get(&r.id).copied(),
                )
            })
            .collect();

        Ok(serde_json::json!({ "wikis": wikis, "group_wikis": [] }))
    }

    /// Legacy wiki1 list — the new model has no legacy wikis, so return an
    /// empty `data` array (matches seahub's `{"data": []}`).
    pub async fn list_wikis_v1(&self) -> Result<serde_json::Value, AppError> {
        Ok(serde_json::json!({ "data": [] }))
    }

    /// Rename a wiki (owner-only).
    pub async fn rename_wiki(
        &self,
        repo_id: &str,
        user_id: i32,
        new_name: &str,
    ) -> Result<(), AppError> {
        self.ensure_wiki_owned(repo_id, user_id).await?;
        let new_name = validate_wiki_name(new_name)?;
        self.repos
            .repo
            .rename_repo(repo_id, &new_name, chrono::Utc::now().timestamp())
            .await?;
        Ok(())
    }

    /// Delete a wiki: delete the underlying library and any publish config.
    pub async fn delete_wiki(&self, repo_id: &str, user_id: i32) -> Result<(), AppError> {
        self.ensure_wiki_owned(repo_id, user_id).await?;
        self.repos.wiki2_publish.delete_by_repo_id(repo_id).await?;
        crate::service::repo::service::RepoService::delete_repo(
            self.db(),
            &self.repos,
            repo_id,
            user_id,
        )
        .await
    }

    /// Publish a wiki under a custom URL. `publish_url` must be 5-30 chars of
    /// `[0-9a-zA-Z-]` and globally unique. Mirrors seahub's validation.
    pub async fn publish_wiki(
        &self,
        repo_id: &str,
        user_id: i32,
        username: &str,
        publish_url: &str,
        enable_server_render: bool,
    ) -> Result<serde_json::Value, AppError> {
        self.ensure_wiki_owned(repo_id, user_id).await?;
        let publish_url = validate_publish_url(publish_url)?;

        if self
            .repos
            .wiki2_publish
            .find_by_publish_url(&publish_url)
            .await?
            .is_some_and(|o| o.repo_id != repo_id)
        {
            return Err(AppError::BadRequest(
                "This custom domain is already in use and cannot be used for your wiki".into(),
            ));
        }

        self.repos
            .wiki2_publish
            .upsert(repo_id, &publish_url, username, enable_server_render)
            .await?;

        Ok(serde_json::json!({
            "publish_url": publish_url,
            "enable_server_render": enable_server_render,
        }))
    }

    /// Cancel a wiki's public publishing.
    pub async fn unpublish_wiki(&self, repo_id: &str, user_id: i32) -> Result<(), AppError> {
        self.ensure_wiki_owned(repo_id, user_id).await?;
        self.repos.wiki2_publish.delete_by_repo_id(repo_id).await
    }

    /// Fetch the publish info for a wiki (used by `GET .../publish/`).
    /// Owner-only, like the other admin endpoints.
    pub async fn publish_info(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<serde_json::Value, AppError> {
        self.ensure_wiki_owned(repo_id, user_id).await?;
        let p = self.repos.wiki2_publish.find_by_repo_id(repo_id).await?;
        Ok(serde_json::json!({
            "publish_url": p.as_ref().map(|x| x.publish_url.clone()).unwrap_or_default(),
            "creator": p.as_ref().map(|x| x.username.clone()).unwrap_or_default(),
            "created_at": p.as_ref().map(|x| x.created_at).unwrap_or(0),
            "visit_count": p.as_ref().map(|x| x.visit_count).unwrap_or(0),
            "enable_server_render": p.as_ref().map(|x| x.enable_server_render).unwrap_or(false),
        }))
    }

    /// Read the wiki config (`_Internal/Wiki/index.json`), defaulting to an
    /// empty structure when absent.
    pub async fn read_wiki_config(&self, repo_id: &str) -> Result<serde_json::Value, AppError> {
        let cfg_path = format!("/{WIKI_CONFIG_PATH}/{WIKI_CONFIG_FILE}");
        match crate::fs::core::download::Downloader::download_file_limited(
            &self.repos,
            repo_id,
            &cfg_path,
            &self.block_store,
            None,
            1 << 20,
        )
        .await
        {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| AppError::Internal(format!("invalid wiki config: {e}"))),
            // Only a missing config falls back to the empty default; real
            // storage/IO errors must propagate instead of being masked.
            Err(AppError::NotFound(_)) => Ok(serde_json::json!({
                "version": 1,
                "navigation": [],
                "pages": [],
            })),
            Err(e) => Err(e),
        }
    }

    /// Persist the wiki config back to `_Internal/Wiki/index.json`.
    pub async fn save_wiki_config(
        &self,
        user_id: i32,
        email: &str,
        repo_id: &str,
        config: &serde_json::Value,
    ) -> Result<(), AppError> {
        let data = serde_json::to_vec(config)?;
        self.file_service
            .upload_file_committed(
                repo_id,
                &format!("/{WIKI_CONFIG_PATH}"),
                WIKI_CONFIG_FILE,
                &data,
                email,
                Some(user_id),
                None,
                false,
                Some(true),
            )
            .await?;
        Ok(())
    }

    /// `GET /api/v2.1/wiki2/{repo_id}/config/` — the wiki record plus its
    /// navigation tree and page list.
    pub async fn get_wiki_config(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<serde_json::Value, AppError> {
        let repo = self.require_wiki(repo_id).await?;
        check_repo_read_permission(self.repos.member.as_ref(), repo_id, user_id).await?;
        let config = self.read_wiki_config(repo_id).await?;
        let owner = self.repos.user.find_by_id(repo.owner_id).await?;
        Ok(serde_json::json!({
            "wiki": {
                "id": repo.id,
                "owner": owner.as_ref().map(|u| u.email.clone()).unwrap_or_default(),
                "name": repo.name,
                "updated_at": infra::common::util::timestamp_rfc3339(repo.updated_at),
                "repo_id": repo.id,
                "wiki_config": config,
            }
        }))
    }

    /// `PUT /api/v2.1/wiki2/{repo_id}/config/` — replace the whole config.
    pub async fn update_wiki_config(
        &self,
        repo_id: &str,
        user_id: i32,
        email: &str,
        config: &serde_json::Value,
    ) -> Result<(), AppError> {
        self.require_wiki(repo_id).await?;
        check_repo_write_permission(self.repos.member.as_ref(), repo_id, user_id).await?;
        self.save_wiki_config(user_id, email, repo_id, config).await
    }

    /// `POST /api/v2.1/wiki2/{repo_id}/pages/` — create a new markdown page and
    /// insert it into the navigation tree.
    pub async fn create_page(
        &self,
        repo_id: &str,
        user_id: i32,
        email: &str,
        page_name: &str,
        current_id: Option<&str>,
        insert_position: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        self.require_wiki(repo_id).await?;
        check_repo_write_permission(self.repos.member.as_ref(), repo_id, user_id).await?;

        // Web forms submit empty strings when the user creates a top-level page;
        // seahub treats falsy current_id/insert_position as absent.
        let current_id = current_id.filter(|s| !s.is_empty());
        let insert_position = insert_position.filter(|s| !s.is_empty());

        let page_name = page_name.trim().to_string();
        if page_name.is_empty() || page_name.contains(['/', '\\']) {
            return Err(AppError::BadRequest("page_name invalid".into()));
        }
        if insert_position.is_some_and(|pos| !["above", "below", "inner"].contains(&pos)) {
            return Err(AppError::BadRequest("insert_position invalid".into()));
        }

        let mut config = self.read_wiki_config(repo_id).await?;
        let doc_uuid = uuid::Uuid::new_v4().to_string();
        let page_id = gen_page_id();
        let page_dir = format!("{WIKI_PAGES_DIR}/{doc_uuid}");

        self.create_dir(user_id, email, repo_id, &page_dir).await?;
        self.upload_markdown(
            user_id,
            email,
            repo_id,
            &page_dir,
            &format!("{page_name}.md"),
            format!("# {page_name}\n\n").as_bytes(),
        )
        .await?;

        // New nav node. Ensure the required keys exist (the wiki config is
        // created with them, but guard against hand-edited configs).
        let cfg_obj = config
            .as_object_mut()
            .ok_or_else(|| AppError::Internal("wiki config is not an object".into()))?;
        cfg_obj
            .entry("navigation")
            .or_insert_with(|| serde_json::Value::Array(vec![]));
        cfg_obj
            .entry("pages")
            .or_insert_with(|| serde_json::Value::Array(vec![]));

        let node = serde_json::json!({ "id": page_id, "type": "page" });
        let navigation = cfg_obj
            .get_mut("navigation")
            .ok_or_else(|| AppError::Internal("missing navigation".into()))?;
        if let Some(current) = current_id {
            let found = insert_nav_node(
                navigation,
                node.clone(),
                current,
                insert_position.unwrap_or("below"),
            );
            if !found {
                return Err(AppError::NotFound("Current page does not exist".into()));
            }
        } else {
            navigation
                .as_array_mut()
                .ok_or_else(|| AppError::Internal("navigation is not an array".into()))?
                .push(node);
        }

        let pages = cfg_obj
            .get_mut("pages")
            .ok_or_else(|| AppError::Internal("missing pages".into()))?;
        pages
            .as_array_mut()
            .ok_or_else(|| AppError::Internal("pages is not an array".into()))?
            .push(serde_json::json!({
                "id": page_id,
                "name": page_name,
                "path": format!("{page_dir}/{page_name}.md"),
                "icon": "",
                "docUuid": doc_uuid,
                "locked": false,
            }));

        self.save_wiki_config(user_id, email, repo_id, &config)
            .await?;

        Ok(serde_json::json!({
            "file_info": {
                "repo_id": repo_id,
                "parent_dir": page_dir,
                "obj_name": format!("{page_name}.md"),
                "doc_uuid": doc_uuid,
                "page_id": page_id,
                "page_name": page_name,
            }
        }))
    }

    /// `PUT /api/v2.1/wiki2/{repo_id}/pages/` — move a page in the navigation.
    pub async fn move_page(
        &self,
        repo_id: &str,
        user_id: i32,
        email: &str,
        target_id: &str,
        moved_id: &str,
        move_position: &str,
    ) -> Result<(), AppError> {
        self.require_wiki(repo_id).await?;
        check_repo_write_permission(self.repos.member.as_ref(), repo_id, user_id).await?;
        if !["move_below", "move_above", "move_into"].contains(&move_position) {
            return Err(AppError::BadRequest("Invalid move_position value".into()));
        }

        let mut config = self.read_wiki_config(repo_id).await?;
        let navigation = config
            .as_object_mut()
            .and_then(|c| c.get_mut("navigation"))
            .ok_or_else(|| AppError::Internal("missing navigation".into()))?;

        let moved = remove_nav_node(navigation, moved_id)
            .ok_or_else(|| AppError::NotFound("Page not found".into()))?;
        // seahub's move vocabulary maps onto the insert vocabulary.
        let position = match move_position {
            "move_above" => "above",
            "move_into" => "inner",
            _ => "below",
        };
        let found = insert_nav_node(navigation, moved, target_id, position);
        if !found {
            return Err(AppError::NotFound("Page not found".into()));
        }

        self.save_wiki_config(user_id, email, repo_id, &config)
            .await
    }

    /// `GET /api/v2.1/wiki2/{repo_id}/page/{page_id}/` — page metadata.
    pub async fn get_page(
        &self,
        repo_id: &str,
        page_id: &str,
        user_id: i32,
    ) -> Result<serde_json::Value, AppError> {
        self.require_wiki(repo_id).await?;
        check_repo_read_permission(self.repos.member.as_ref(), repo_id, user_id).await?;
        let config = self.read_wiki_config(repo_id).await?;
        let page = find_page(&config, page_id)
            .ok_or_else(|| AppError::NotFound("page not found".into()))?;
        Ok(page.clone())
    }

    /// `DELETE /api/v2.1/wiki2/{repo_id}/page/{page_id}/` — delete a page.
    pub async fn delete_page(
        &self,
        repo_id: &str,
        user_id: i32,
        email: &str,
        page_id: &str,
    ) -> Result<(), AppError> {
        self.require_wiki(repo_id).await?;
        check_repo_write_permission(self.repos.member.as_ref(), repo_id, user_id).await?;

        let mut config = self.read_wiki_config(repo_id).await?;
        // Check the page's file lock before removing it (seahub's delete guard).
        let page_path = find_page(&config, page_id)
            .and_then(|p| p.get("path").and_then(|v| v.as_str()))
            .map(|s| s.to_string());
        if let Some(path) = page_path {
            self.check_page_lock(repo_id, &path, user_id).await?;
        }
        let navigation = config
            .as_object_mut()
            .and_then(|c| c.get_mut("navigation"))
            .ok_or_else(|| AppError::Internal("missing navigation".into()))?;
        if remove_nav_node(navigation, page_id).is_none() {
            return Err(AppError::NotFound("Page not found".into()));
        }
        let pages = config
            .as_object_mut()
            .and_then(|c| c.get_mut("pages"))
            .ok_or_else(|| AppError::Internal("missing pages".into()))?;
        if let Some(arr) = pages.as_array_mut() {
            arr.retain(|p| p.get("id").and_then(|v| v.as_str()) != Some(page_id));
        }
        self.save_wiki_config(user_id, email, repo_id, &config)
            .await
    }

    /// `PUT /api/v2.1/wiki2/{repo_id}/page/{page_id}/` — lock / unlock a page.
    pub async fn set_page_locked(
        &self,
        repo_id: &str,
        user_id: i32,
        email: &str,
        page_id: &str,
        locked: bool,
    ) -> Result<(), AppError> {
        self.require_wiki(repo_id).await?;
        check_repo_write_permission(self.repos.member.as_ref(), repo_id, user_id).await?;
        let mut config = self.read_wiki_config(repo_id).await?;
        let page = find_page_mut(&mut config, page_id)
            .ok_or_else(|| AppError::NotFound("page not found".into()))?;
        let path = page
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::NotFound("page path missing".into()))?
            .to_string();
        // A real file lock (seahub's lock_file/unlock_file), not just the config flag.
        let operation = if locked { "lock" } else { "unlock" };
        self.file_service
            .lock_file(repo_id, &path, operation, email)
            .await?;
        page["locked"] = serde_json::Value::Bool(locked);
        self.save_wiki_config(user_id, email, repo_id, &config)
            .await
    }

    /// Reject the operation if the page file is locked by a different user
    /// (mirrors seahub's `check_file_lock`). The lock owner may still proceed.
    async fn check_page_lock(
        &self,
        repo_id: &str,
        path: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        if let Some(lock) = self
            .repos
            .locked_file
            .find_by_repo_and_path(repo_id, path)
            .await?
            && lock.user_id != user_id
        {
            return Err(AppError::Locked(path.to_string()));
        }
        Ok(())
    }

    /// `PUT /api/v2.1/wiki2/{repo_id}/page/{page_id}/config/` — page name/icon/cover.
    pub async fn update_page_config(
        &self,
        repo_id: &str,
        user_id: i32,
        email: &str,
        page_id: &str,
        page_name: Option<&str>,
        page_icon: Option<&str>,
        page_cover: Option<&str>,
    ) -> Result<(), AppError> {
        self.require_wiki(repo_id).await?;
        check_repo_write_permission(self.repos.member.as_ref(), repo_id, user_id).await?;
        if page_name.is_none() && page_icon.is_none() && page_cover.is_none() {
            return Err(AppError::BadRequest(
                "At least one of page_name, page_icon or page_cover is required.".into(),
            ));
        }
        let mut config = self.read_wiki_config(repo_id).await?;
        let page = find_page_mut(&mut config, page_id)
            .ok_or_else(|| AppError::NotFound("page not found".into()))?;
        if let Some(name) = page_name {
            page["name"] = serde_json::Value::String(name.to_string());
        }
        if let Some(icon) = page_icon {
            page["icon"] = serde_json::Value::String(icon.to_string());
        }
        if let Some(cover) = page_cover {
            page["cover_img_url"] = serde_json::Value::String(cover.to_string());
        }
        self.save_wiki_config(user_id, email, repo_id, &config)
            .await
    }

    /// Resolve a page id to its markdown content (used by the web UI).
    pub async fn get_page_content(
        &self,
        repo_id: &str,
        page_id: &str,
    ) -> Result<Option<(String, Vec<u8>)>, AppError> {
        let config = self.read_wiki_config(repo_id).await?;
        self.get_page_content_from_config(repo_id, page_id, &config)
            .await
    }

    /// Like [`get_page_content`](Self::get_page_content) but reuses an already
    /// loaded config (the web UI reads it once for the whole page render).
    pub async fn get_page_content_from_config(
        &self,
        repo_id: &str,
        page_id: &str,
        config: &serde_json::Value,
    ) -> Result<Option<(String, Vec<u8>)>, AppError> {
        let page = match find_page(config, page_id) {
            Some(p) => p,
            None => return Ok(None),
        };
        let path = match page.get("path").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return Ok(None),
        };
        let data = crate::fs::core::download::Downloader::download_file_limited(
            &self.repos,
            repo_id,
            &path,
            &self.block_store,
            None,
            1 << 20,
        )
        .await?;
        Ok(Some((path, data)))
    }

    /// Persist a page's markdown content (web UI edit-save).
    pub async fn save_page_content(
        &self,
        repo_id: &str,
        user_id: i32,
        email: &str,
        page_id: &str,
        content: &[u8],
    ) -> Result<(), AppError> {
        self.require_wiki(repo_id).await?;
        check_repo_write_permission(self.repos.member.as_ref(), repo_id, user_id).await?;
        let config = self.read_wiki_config(repo_id).await?;
        let page = find_page(&config, page_id)
            .ok_or_else(|| AppError::NotFound("page not found".into()))?;
        let path = page
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::NotFound("page path missing".into()))?;
        self.check_page_lock(repo_id, path, user_id).await?;
        let dir = infra::common::util::parent_path_from(path).to_string();
        let name = infra::common::util::basename(path).to_string();
        self.upload_markdown(user_id, email, repo_id, &dir, &name, content)
            .await
    }

    /// Require the repo to be a wiki (without ownership — used by config/page
    /// endpoints where repo membership governs access).
    async fn require_wiki(&self, repo_id: &str) -> Result<infra::entity::repo::Model, AppError> {
        let repo = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("wiki not found".into()))?;
        if repo.r#type != "wiki" {
            return Err(AppError::NotFound("wiki not found".into()));
        }
        Ok(repo)
    }

    /// Verify a repo exists, is a wiki, and the user owns it.
    async fn ensure_wiki_owned(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<infra::entity::repo::Model, AppError> {
        let repo = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("wiki not found".into()))?;
        if repo.r#type != "wiki" {
            return Err(AppError::NotFound("wiki not found".into()));
        }
        if repo.owner_id != user_id {
            return Err(AppError::Forbidden);
        }
        Ok(repo)
    }

    /// Create a directory inside a repo (handles empty repos).
    async fn create_dir(
        &self,
        user_id: i32,
        email: &str,
        repo_id: &str,
        path: &str,
    ) -> Result<(), AppError> {
        crate::service::fs::dir::create_dir_by_path(
            self.db(),
            &self.repos,
            email,
            user_id,
            repo_id,
            path,
        )
        .await
    }

    /// Write a markdown page file into a repo directory.
    async fn upload_markdown(
        &self,
        user_id: i32,
        email: &str,
        repo_id: &str,
        target_dir: &str,
        filename: &str,
        data: &[u8],
    ) -> Result<(), AppError> {
        self.file_service
            .upload_file_committed(
                repo_id,
                target_dir,
                filename,
                data,
                email,
                Some(user_id),
                None,
                false,
                Some(true),
            )
            .await?;
        Ok(())
    }
}

/// Build the base wiki2 JSON item (shared fields).
fn wiki2_json(
    repo_id: &str,
    name: &str,
    owner: &str,
    permission: &str,
    type_: &str,
    is_published: bool,
) -> serde_json::Value {
    serde_json::json!({
        "id": repo_id,
        "owner": owner,
        "name": name,
        "updated_at": infra::common::util::timestamp_rfc3339(chrono::Utc::now().timestamp()),
        "repo_id": repo_id,
        "type": type_,
        "permission": permission,
        "is_published": is_published,
        "color": "",
        "icon": "",
    })
}

/// Build one wiki2 list item from a repo model, using preloaded publish config
/// and owner info (avoids a per-wiki DB round-trip when listing).
fn build_wiki_item(
    repo: &infra::entity::repo::Model,
    owner_email: &str,
    owner_nickname: &str,
    type_: &str,
    permission: &str,
    publish: Option<&infra::entity::wiki2_publish::Model>,
) -> serde_json::Value {
    let is_published = publish.is_some();
    let public_url_suffix = publish.map(|p| p.publish_url.clone()).unwrap_or_default();
    let public_url = if is_published {
        format!("/wiki/publish/{public_url_suffix}")
    } else {
        String::new()
    };
    let enable_server_render = publish.map(|p| p.enable_server_render).unwrap_or(false);

    let mut v = wiki2_json(
        &repo.id,
        &repo.name,
        owner_email,
        permission,
        type_,
        is_published,
    );
    let obj = v.as_object_mut().expect("wiki2_json is an object");
    obj.insert(
        "owner_nickname".into(),
        serde_json::Value::String(owner_nickname.to_string()),
    );
    obj.insert(
        "public_url_suffix".into(),
        serde_json::Value::String(public_url_suffix),
    );
    obj.insert("public_url".into(), serde_json::Value::String(public_url));
    obj.insert(
        "enable_server_render".into(),
        serde_json::Value::Bool(enable_server_render),
    );
    v
}

fn validate_wiki_name(name: &str) -> Result<String, AppError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 255 || trimmed.contains('/') {
        return Err(AppError::BadRequest("invalid wiki name".into()));
    }
    Ok(trimmed.to_string())
}

/// seahub publishes require 5-30 chars of `[0-9a-zA-Z-]`.
fn validate_publish_url(url: &str) -> Result<String, AppError> {
    let url = url.trim();
    if url.len() < 5 || url.len() > 30 {
        return Err(AppError::BadRequest(
            "The custom part of URL should have 5-30 characters.".into(),
        ));
    }
    if !url.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(AppError::BadRequest("URL is invalid".into()));
    }
    Ok(url.to_string())
}

/// Whether a repo-relative path points into a wiki repo's hidden internal
/// storage (`_Internal` config dir, `/wiki-pages` page files). Used to keep
/// those out of downloads and searches.
pub fn is_hidden_wiki_path(path: &str) -> bool {
    let p = path.trim_start_matches('/');
    p == "_Internal"
        || p.starts_with("_Internal/")
        || p == "wiki-pages"
        || p.starts_with("wiki-pages/")
}

/// 4-char random base62 page id (seahub's `gen_unique_id`).
fn gen_page_id() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    use rand::RngExt;
    let mut out = String::with_capacity(4);
    let mut rng = rand::rng();
    for _ in 0..4 {
        let idx = rng.random_range(0..CHARSET.len());
        out.push(CHARSET[idx] as char);
    }
    out
}

fn requester_email_nickname(email: &str) -> String {
    email.split('@').next().unwrap_or("").to_string()
}

// ── Navigation tree helpers (operate on the wiki config JSON) ──────────────

/// Insert `node` into the nav tree relative to `current_id`
/// (`above` / `below` / `inner`). Returns false if `current_id` is missing.
fn insert_nav_node(
    navigation: &mut serde_json::Value,
    node: serde_json::Value,
    current_id: &str,
    position: &str,
) -> bool {
    let arr = match navigation.as_array_mut() {
        Some(a) => a,
        None => return false,
    };
    // Direct sibling/child match at this level.
    for i in 0..arr.len() {
        let id_matches = arr[i]
            .get("id")
            .and_then(|v| v.as_str())
            .is_some_and(|id| id == current_id);
        if id_matches {
            match position {
                "above" => arr.insert(i, node),
                "inner" => {
                    if arr[i].get("children").is_none() {
                        arr[i]["children"] = serde_json::Value::Array(vec![]);
                    }
                    arr[i]["children"]
                        .as_array_mut()
                        .expect("children must be an array")
                        .push(node);
                    return true;
                }
                _ => arr.insert(i + 1, node),
            }
            return true;
        }
    }
    // Recurse into children.
    for item in arr.iter_mut() {
        if let Some(children) = item.get_mut("children")
            && insert_nav_node(children, node.clone(), current_id, position)
        {
            return true;
        }
    }
    false
}

/// Remove a nav node (and its subtree) by id, returning the removed node.
fn remove_nav_node(navigation: &mut serde_json::Value, page_id: &str) -> Option<serde_json::Value> {
    let arr = navigation.as_array_mut()?;
    for i in 0..arr.len() {
        let id_matches = arr[i]
            .get("id")
            .and_then(|v| v.as_str())
            .is_some_and(|id| id == page_id);
        if id_matches {
            return Some(arr.remove(i));
        }
    }
    for item in arr.iter_mut() {
        if let Some(children) = item.get_mut("children")
            && let Some(node) = remove_nav_node(children, page_id)
        {
            return Some(node);
        }
    }
    None
}

/// Find a page entry in the config's `pages` array.
pub(crate) fn find_page<'a>(
    config: &'a serde_json::Value,
    page_id: &str,
) -> Option<&'a serde_json::Value> {
    config
        .get("pages")
        .and_then(|pages| pages.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|p| p.get("id").and_then(|v| v.as_str()) == Some(page_id))
        })
}

/// Mutably find a page entry in the config's `pages` array.
pub(crate) fn find_page_mut<'a>(
    config: &'a mut serde_json::Value,
    page_id: &str,
) -> Option<&'a mut serde_json::Value> {
    config
        .get_mut("pages")
        .and_then(|pages| pages.as_array_mut())
        .and_then(|arr| {
            arr.iter_mut()
                .find(|p| p.get("id").and_then(|v| v.as_str()) == Some(page_id))
        })
}
