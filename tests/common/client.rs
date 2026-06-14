use reqwest::Client;

pub struct TestClient {
    client: Client,
    base_url: String,
}

impl TestClient {
    /// Create a test client that does NOT track cookies (default for API tests).
    pub fn new(base_url: &str) -> Self {
        Self {
            client: Client::builder().no_proxy().build().unwrap(),
            base_url: base_url.to_string(),
        }
    }

    /// Create a test client WITH cookie store enabled.
    ///
    /// This simulates a browser session: `Set-Cookie` responses are stored and
    /// automatically sent as `Cookie` headers on subsequent requests.
    /// Used for Web UI E2E tests (login → cookie → access protected pages).
    pub fn new_with_cookies(base_url: &str) -> Self {
        Self {
            client: Client::builder()
                .no_proxy()
                .cookie_store(true)
                .build()
                .unwrap(),
            base_url: base_url.to_string(),
        }
    }

    // ========== Generic request helpers ==========

    /// Build a URL from a path (must start with `/`).
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// GET request with optional Bearer token.
    pub async fn get(&self, path: &str, token: Option<&str>) -> reqwest::Response {
        let mut req = self.client.get(self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.send().await.unwrap()
    }

    /// POST with JSON body and optional Bearer token.
    pub async fn post_json(
        &self,
        path: &str,
        token: Option<&str>,
        body: &serde_json::Value,
    ) -> reqwest::Response {
        let mut req = self.client.post(self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.json(body).send().await.unwrap()
    }

    /// POST with form body and optional Bearer token.
    pub async fn post_form(
        &self,
        path: &str,
        token: Option<&str>,
        fields: &[(&str, &str)],
    ) -> reqwest::Response {
        let mut req = self.client.post(self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.form(fields).send().await.unwrap()
    }

    /// POST with raw bytes body and optional Bearer token.
    pub async fn post_bytes(
        &self,
        path: &str,
        token: Option<&str>,
        body: Vec<u8>,
    ) -> reqwest::Response {
        let mut req = self.client.post(self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.body(body).send().await.unwrap()
    }

    /// PUT with JSON body and optional Bearer token.
    pub async fn put_json(
        &self,
        path: &str,
        token: Option<&str>,
        body: &serde_json::Value,
    ) -> reqwest::Response {
        let mut req = self.client.put(self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.json(body).send().await.unwrap()
    }

    /// PUT with raw bytes body and optional Bearer token.
    pub async fn put_bytes(
        &self,
        path: &str,
        token: Option<&str>,
        body: Vec<u8>,
    ) -> reqwest::Response {
        let mut req = self.client.put(self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.body(body).send().await.unwrap()
    }

    /// DELETE request with optional Bearer token.
    pub async fn delete(&self, path: &str, token: Option<&str>) -> reqwest::Response {
        let mut req = self.client.delete(self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.send().await.unwrap()
    }

    /// DELETE with form body and optional Bearer token.
    pub async fn delete_form(
        &self,
        path: &str,
        token: Option<&str>,
        fields: &[(&str, &str)],
    ) -> reqwest::Response {
        let mut req = self.client.delete(self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.form(fields).send().await.unwrap()
    }

    /// GET request authenticated via Seafile-Repo-Token header (sync protocol).
    pub async fn get_sync(&self, path: &str, token: &str) -> reqwest::Response {
        self.client
            .get(self.url(path))
            .header("Seafile-Repo-Token", token)
            .send()
            .await
            .unwrap()
    }

    /// POST with form body, authenticated via Seafile-Repo-Token header.
    pub async fn post_sync_form(
        &self,
        path: &str,
        sync_token: &str,
        fields: &[(&str, &str)],
    ) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .header("Seafile-Repo-Token", sync_token)
            .form(fields)
            .send()
            .await
            .unwrap()
    }

    /// POST with JSON body, authenticated via Seafile-Repo-Token header.
    pub async fn post_sync_json(
        &self,
        path: &str,
        sync_token: &str,
        body: &serde_json::Value,
    ) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .header("Seafile-Repo-Token", sync_token)
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(body).unwrap())
            .send()
            .await
            .unwrap()
    }

    /// POST with JSON body as raw bytes, NO Content-Type header.
    ///
    /// This matches how seaf-daemon sends POST requests via curl, which does
    /// not set Content-Type by default. Tests using this method verify that
    /// endpoints accept requests without a Content-Type header.
    pub async fn post_sync_raw(
        &self,
        path: &str,
        sync_token: &str,
        body: &serde_json::Value,
    ) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .header("Seafile-Repo-Token", sync_token)
            .body(serde_json::to_vec(body).unwrap())
            .send()
            .await
            .unwrap()
    }

    /// PUT with body bytes, authenticated via Seafile-Repo-Token header.
    pub async fn put_sync(&self, path: &str, sync_token: &str, body: Vec<u8>) -> reqwest::Response {
        self.client
            .put(self.url(path))
            .header("Seafile-Repo-Token", sync_token)
            .body(body)
            .send()
            .await
            .unwrap()
    }

    /// POST with raw bytes, authenticated via Seafile-Repo-Token header.
    pub async fn post_sync_bytes(
        &self,
        path: &str,
        sync_token: &str,
        body: Vec<u8>,
    ) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .header("Seafile-Repo-Token", sync_token)
            .body(body)
            .send()
            .await
            .unwrap()
    }

    // ========== API Endpoints ==========

    pub async fn login(&self, username: &str, password: &str) -> reqwest::Response {
        self.client
            .post(format!("{}/api2/auth-token/", self.base_url))
            .form(&[("username", username), ("password", password)])
            .send()
            .await
            .unwrap()
    }

    /// Login with JSON body (matching the JSON support added to the API).
    pub async fn login_json(&self, username: &str, password: &str) -> reqwest::Response {
        self.client
            .post(format!("{}/api2/auth-token/", self.base_url))
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "username": username,
                "password": password,
            }))
            .send()
            .await
            .unwrap()
    }

    /// Login with multipart/form-data body (matching seadroid's request format).
    pub async fn login_multipart(&self, username: &str, password: &str) -> reqwest::Response {
        let form = reqwest::multipart::Form::new()
            .text("username", username.to_string())
            .text("password", password.to_string())
            .text("platform", "android")
            .text("device_id", "test-device-123")
            .text("device_name", "Test Device")
            .text("client_version", "3.0.0");

        self.client
            .post(format!("{}/api2/auth-token/", self.base_url))
            .multipart(form)
            .send()
            .await
            .unwrap()
    }

    pub async fn login_with_otp(
        &self,
        username: &str,
        password: &str,
        otp: &str,
    ) -> reqwest::Response {
        self.client
            .post(format!("{}/api2/auth-token/", self.base_url))
            .form(&[("username", username), ("password", password)])
            .header("X-SEAFILE-OTP", otp)
            .send()
            .await
            .unwrap()
    }

    pub async fn login_with_otp_and_trust_device(
        &self,
        username: &str,
        password: &str,
        otp: &str,
    ) -> reqwest::Response {
        self.client
            .post(format!("{}/api2/auth-token/", self.base_url))
            .form(&[("username", username), ("password", password)])
            .header("X-SEAFILE-OTP", otp)
            .header("X-SEAFILE-2FA-TRUST-DEVICE", "1")
            .send()
            .await
            .unwrap()
    }

    pub async fn login_with_s2fa(
        &self,
        username: &str,
        password: &str,
        s2fa_token: &str,
    ) -> reqwest::Response {
        self.client
            .post(format!("{}/api2/auth-token/", self.base_url))
            .form(&[("username", username), ("password", password)])
            .header("X-SEAFILE-S2FA", s2fa_token)
            .send()
            .await
            .unwrap()
    }

    pub async fn ping(&self, token: &str) -> reqwest::Response {
        self.client
            .get(format!("{}/api2/auth/ping/", self.base_url))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    pub async fn create_repo(&self, token: &str, name: &str) -> reqwest::Response {
        self.client
            .post(format!("{}/api2/repos/", self.base_url))
            .bearer_auth(token)
            .form(&serde_json::json!({"name": name}))
            .send()
            .await
            .unwrap()
    }

    /// POST /api2/repos/ with multipart body (Android client format).
    pub async fn create_repo_multipart(&self, token: &str, name: &str) -> reqwest::Response {
        let form = reqwest::multipart::Form::new().text("name", name.to_string());
        self.client
            .post(format!("{}/api2/repos/", self.base_url))
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await
            .unwrap()
    }

    /// POST /api2/repos/ with multipart body, including description (Android client format).
    pub async fn create_repo_multipart_with_desc(
        &self,
        token: &str,
        name: &str,
        desc: &str,
    ) -> reqwest::Response {
        let form = reqwest::multipart::Form::new()
            .text("name", name.to_string())
            .text("desc", desc.to_string());
        self.client
            .post(format!("{}/api2/repos/", self.base_url))
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await
            .unwrap()
    }

    pub async fn list_repos(&self, token: &str) -> reqwest::Response {
        self.client
            .get(format!("{}/api2/repos/", self.base_url))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    pub async fn get_repo(&self, token: &str, repo_id: &str) -> reqwest::Response {
        self.client
            .get(format!("{}/api2/repos/{}/", self.base_url, repo_id))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    pub async fn delete_repo(&self, token: &str, repo_id: &str) -> reqwest::Response {
        self.client
            .delete(format!("{}/api2/repos/{}/", self.base_url, repo_id))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    pub async fn rename_repo(
        &self,
        token: &str,
        repo_id: &str,
        new_name: &str,
    ) -> reqwest::Response {
        self.client
            .post(format!(
                "{}/api2/repos/{}/?op=rename",
                self.base_url, repo_id
            ))
            .bearer_auth(token)
            .form(&serde_json::json!({"repo_name": new_name}))
            .send()
            .await
            .unwrap()
    }

    /// POST /api2/repos/{repo_id}/?op=rename with multipart body (Android
    /// client format — @Multipart + @PartMap with name="repo_name").
    pub async fn rename_repo_multipart(
        &self,
        token: &str,
        repo_id: &str,
        new_name: &str,
    ) -> reqwest::Response {
        let form = reqwest::multipart::Form::new().text("repo_name", new_name.to_string());
        self.client
            .post(format!(
                "{}/api2/repos/{}/?op=rename",
                self.base_url, repo_id
            ))
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await
            .unwrap()
    }

    pub async fn download_info(&self, token: &str, repo_id: &str) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/api2/repos/{}/download-info/",
                self.base_url, repo_id
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    pub async fn upload_file(
        &self,
        token: &str,
        repo_id: &str,
        parent_dir: &str,
        filename: &str,
        data: &[u8],
    ) -> reqwest::Response {
        let file_part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(filename.to_string())
            .mime_str("application/octet-stream")
            .unwrap();

        let form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("parent_dir", parent_dir.to_string())
            .text("replace", "1".to_string());

        self.client
            .post(format!("{}/api2/repos/{}/file/", self.base_url, repo_id))
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await
            .unwrap()
    }

    pub async fn upload_file_with_replace(
        &self,
        token: &str,
        repo_id: &str,
        parent_dir: &str,
        filename: &str,
        data: &[u8],
        replace: bool,
    ) -> reqwest::Response {
        let file_part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(filename.to_string())
            .mime_str("application/octet-stream")
            .unwrap();

        let form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("parent_dir", parent_dir.to_string())
            .text(
                "replace",
                if replace {
                    "1".to_string()
                } else {
                    "0".to_string()
                },
            );

        self.client
            .post(format!("{}/api2/repos/{}/file/", self.base_url, repo_id))
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await
            .unwrap()
    }

    pub async fn download_file(&self, token: &str, repo_id: &str, path: &str) -> reqwest::Response {
        // Step A: get the download URL (now returns a JSON-quoted URL string)
        let link_resp = self
            .client
            .get(format!(
                "{}/api2/repos/{}/file/?p={}&reuse=0",
                self.base_url, repo_id, path
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert!(link_resp.status().is_success(), "get download link failed");
        let url_str: String = link_resp.json().await.unwrap_or_default();
        assert!(
            url_str.starts_with("http"),
            "download URL should start with http, got: {url_str}"
        );

        // Step B: follow the download URL to get the raw file bytes
        self.client.get(&url_str).send().await.unwrap()
    }

    pub async fn list_dir(&self, token: &str, repo_id: &str, path: &str) -> reqwest::Response {
        self.list_dir_with_params(token, repo_id, path, None, None)
            .await
    }

    /// List directory with optional `recursive` and `t` query parameters.
    ///
    /// `recursive`: Some("1") for recursive listing, None otherwise.
    /// `t`: Some("f") for files only, Some("d") for dirs only, None for all.
    pub async fn list_dir_with_params(
        &self,
        token: &str,
        repo_id: &str,
        path: &str,
        recursive: Option<&str>,
        t: Option<&str>,
    ) -> reqwest::Response {
        let mut url = format!("{}/api2/repos/{}/dir/?p={}", self.base_url, repo_id, path);
        if let Some(r) = recursive {
            url.push_str(&format!("&recursive={}", r));
        }
        if let Some(ty) = t {
            url.push_str(&format!("&t={}", ty));
        }
        self.client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    pub async fn create_dir(&self, token: &str, repo_id: &str, path: &str) -> reqwest::Response {
        self.client
            .post(format!("{}/api2/repos/{}/dir/", self.base_url, repo_id))
            .bearer_auth(token)
            .json(&serde_json::json!({"p": path}))
            .send()
            .await
            .unwrap()
    }

    /// POST /api2/repos/{repo_id}/dir/?p={path} with multipart body
    /// `operation=mkdir` (mimicking seadroid AlbumScanHelper.mkdirRemote).
    pub async fn create_dir_multipart(
        &self,
        token: &str,
        repo_id: &str,
        path: &str,
    ) -> reqwest::Response {
        let form = reqwest::multipart::Form::new()
            .text("operation", "mkdir")
            .text("create_parents", "true");
        self.client
            .post(format!(
                "{}/api2/repos/{}/dir/?p={}",
                self.base_url, repo_id, path
            ))
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await
            .unwrap()
    }

    // ========== Web UI Endpoints ==========

    /// GET a URL without any auth (for login page, etc.).
    pub async fn get_ui(&self, path: &str) -> reqwest::Response {
        self.client.get(self.url(path)).send().await.unwrap()
    }

    /// POST form data without any auth (for login submission).
    pub async fn post_ui_form(&self, path: &str, fields: &[(&str, &str)]) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .form(fields)
            .send()
            .await
            .unwrap()
    }

    /// POST multipart form data (for file upload).
    pub async fn post_ui_multipart(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .multipart(form)
            .send()
            .await
            .unwrap()
    }

    /// POST with multipart body and optional Bearer token.
    pub async fn post_multipart(
        &self,
        path: &str,
        token: Option<&str>,
        form: reqwest::multipart::Form,
    ) -> reqwest::Response {
        let mut req = self.client.post(self.url(path));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.multipart(form).send().await.unwrap()
    }

    // ========== Sync Protocol Endpoints ==========

    pub async fn protocol_version(&self) -> reqwest::Response {
        self.client
            .get(format!("{}/seafhttp/protocol-version", self.base_url))
            .send()
            .await
            .unwrap()
    }

    pub async fn head_commits_multi(&self, repo_ids: &[&str]) -> reqwest::Response {
        self.client
            .post(format!(
                "{}/seafhttp/repo/head-commits-multi/",
                self.base_url
            ))
            .json(&repo_ids)
            .send()
            .await
            .unwrap()
    }

    pub async fn get_head_commit(&self, token: &str, repo_id: &str) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/seafhttp/repo/{}/commit/HEAD",
                self.base_url, repo_id
            ))
            .header("Seafile-Repo-Token", token)
            .send()
            .await
            .unwrap()
    }

    pub async fn get_commit(
        &self,
        token: &str,
        repo_id: &str,
        commit_id: &str,
    ) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/seafhttp/repo/{}/commit/{}",
                self.base_url, repo_id, commit_id
            ))
            .header("Seafile-Repo-Token", token)
            .send()
            .await
            .unwrap()
    }

    pub async fn put_commit(
        &self,
        token: &str,
        repo_id: &str,
        commit_id: &str,
        data: Vec<u8>,
    ) -> reqwest::Response {
        self.client
            .put(format!(
                "{}/seafhttp/repo/{}/commit/{}",
                self.base_url, repo_id, commit_id
            ))
            .header("Seafile-Repo-Token", token)
            .body(data)
            .send()
            .await
            .unwrap()
    }

    pub async fn update_branch(&self, token: &str, repo_id: &str, head: &str) -> reqwest::Response {
        self.client
            .put(format!(
                "{}/seafhttp/repo/{}/commit/HEAD?head={}",
                self.base_url, repo_id, head
            ))
            .header("Seafile-Repo-Token", token)
            .send()
            .await
            .unwrap()
    }

    pub async fn fs_id_list(
        &self,
        token: &str,
        repo_id: &str,
        server_head: &str,
    ) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/seafhttp/repo/{}/fs-id-list/?server-head={}",
                self.base_url, repo_id, server_head
            ))
            .header("Seafile-Repo-Token", token)
            .send()
            .await
            .unwrap()
    }

    pub async fn fs_id_list_with_client(
        &self,
        token: &str,
        repo_id: &str,
        server_head: &str,
        client_head: &str,
    ) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/seafhttp/repo/{}/fs-id-list/?server-head={}&client-head={}",
                self.base_url, repo_id, server_head, client_head
            ))
            .header("Seafile-Repo-Token", token)
            .send()
            .await
            .unwrap()
    }

    pub async fn fs_id_list_with_dir_only(
        &self,
        token: &str,
        repo_id: &str,
        server_head: &str,
        dir_only: &str,
    ) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/seafhttp/repo/{}/fs-id-list/?server-head={}&dir-only={}",
                self.base_url, repo_id, server_head, dir_only
            ))
            .header("Seafile-Repo-Token", token)
            .send()
            .await
            .unwrap()
    }

    pub async fn fs_id_list_with_client_and_dir_only(
        &self,
        token: &str,
        repo_id: &str,
        server_head: &str,
        client_head: &str,
        dir_only: &str,
    ) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/seafhttp/repo/{}/fs-id-list/?server-head={}&client-head={}&dir-only={}",
                self.base_url, repo_id, server_head, client_head, dir_only
            ))
            .header("Seafile-Repo-Token", token)
            .send()
            .await
            .unwrap()
    }

    pub async fn pack_fs(&self, token: &str, repo_id: &str, fs_ids: &[&str]) -> reqwest::Response {
        self.client
            .post(format!(
                "{}/seafhttp/repo/{}/pack-fs/",
                self.base_url, repo_id
            ))
            .header("Seafile-Repo-Token", token)
            .json(&fs_ids)
            .send()
            .await
            .unwrap()
    }

    pub async fn check_fs(&self, token: &str, repo_id: &str, fs_ids: &[&str]) -> reqwest::Response {
        self.client
            .post(format!(
                "{}/seafhttp/repo/{}/check-fs/",
                self.base_url, repo_id
            ))
            .header("Seafile-Repo-Token", token)
            .json(&fs_ids)
            .send()
            .await
            .unwrap()
    }

    pub async fn recv_fs(&self, token: &str, repo_id: &str, data: Vec<u8>) -> reqwest::Response {
        self.client
            .post(format!(
                "{}/seafhttp/repo/{}/recv-fs/",
                self.base_url, repo_id
            ))
            .header("Seafile-Repo-Token", token)
            .body(data)
            .send()
            .await
            .unwrap()
    }

    pub async fn check_blocks(
        &self,
        token: &str,
        repo_id: &str,
        block_ids: &[&str],
    ) -> reqwest::Response {
        self.client
            .post(format!(
                "{}/seafhttp/repo/{}/check-blocks/",
                self.base_url, repo_id
            ))
            .header("Seafile-Repo-Token", token)
            .json(&block_ids)
            .send()
            .await
            .unwrap()
    }

    pub async fn get_block(&self, token: &str, repo_id: &str, block_id: &str) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/seafhttp/repo/{}/block/{}",
                self.base_url, repo_id, block_id
            ))
            .header("Seafile-Repo-Token", token)
            .send()
            .await
            .unwrap()
    }

    pub async fn put_block(
        &self,
        token: &str,
        repo_id: &str,
        block_id: &str,
        data: Vec<u8>,
    ) -> reqwest::Response {
        self.client
            .put(format!(
                "{}/seafhttp/repo/{}/block/{}",
                self.base_url, repo_id, block_id
            ))
            .header("Seafile-Repo-Token", token)
            .body(data)
            .send()
            .await
            .unwrap()
    }

    pub async fn block_map(&self, token: &str, repo_id: &str, file_id: &str) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/seafhttp/repo/{}/block-map/{}",
                self.base_url, repo_id, file_id
            ))
            .header("Seafile-Repo-Token", token)
            .send()
            .await
            .unwrap()
    }

    pub async fn permission_check(
        &self,
        token: &str,
        repo_id: &str,
        op: &str,
    ) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/seafhttp/repo/{}/permission-check/?op={}",
                self.base_url, repo_id, op
            ))
            .header("Seafile-Repo-Token", token)
            .send()
            .await
            .unwrap()
    }

    // ========== Batch Operations ==========

    /// POST /api2/repos/{repo_id}/fileops/delete/
    pub async fn batch_delete(
        &self,
        token: &str,
        repo_id: &str,
        parent_dir: &str,
        file_names: &[&str],
    ) -> reqwest::Response {
        let file_names_str = file_names.join(":");
        self.client
            .post(format!(
                "{}/api2/repos/{}/fileops/delete/?p={}",
                self.base_url, repo_id, parent_dir
            ))
            .bearer_auth(token)
            .form(&[("file_names", file_names_str.as_str())])
            .send()
            .await
            .unwrap()
    }

    /// POST /api2/repos/{repo_id}/fileops/delete/ with reloaddir=true
    pub async fn batch_delete_with_dir(
        &self,
        token: &str,
        repo_id: &str,
        parent_dir: &str,
        file_names: &[&str],
    ) -> reqwest::Response {
        let file_names_str = file_names.join(":");
        self.client
            .post(format!(
                "{}/api2/repos/{}/fileops/delete/?p={}&reloaddir=true",
                self.base_url, repo_id, parent_dir
            ))
            .bearer_auth(token)
            .form(&[("file_names", file_names_str.as_str())])
            .send()
            .await
            .unwrap()
    }

    /// POST /api2/repos/{repo_id}/fileops/copy/
    pub async fn batch_copy(
        &self,
        token: &str,
        repo_id: &str,
        parent_dir: &str,
        file_names: &[&str],
        dst_repo: &str,
        dst_dir: &str,
    ) -> reqwest::Response {
        let file_names_str = file_names.join(":");
        self.client
            .post(format!(
                "{}/api2/repos/{}/fileops/copy/?p={}",
                self.base_url, repo_id, parent_dir
            ))
            .bearer_auth(token)
            .form(&[
                ("file_names", file_names_str.as_str()),
                ("dst_repo", dst_repo),
                ("dst_dir", dst_dir),
            ])
            .send()
            .await
            .unwrap()
    }

    /// POST /api2/repos/{repo_id}/fileops/move/
    pub async fn batch_move(
        &self,
        token: &str,
        repo_id: &str,
        parent_dir: &str,
        file_names: &[&str],
        dst_repo: &str,
        dst_dir: &str,
    ) -> reqwest::Response {
        let file_names_str = file_names.join(":");
        self.client
            .post(format!(
                "{}/api2/repos/{}/fileops/move/?p={}",
                self.base_url, repo_id, parent_dir
            ))
            .bearer_auth(token)
            .form(&[
                ("file_names", file_names_str.as_str()),
                ("dst_repo", dst_repo),
                ("dst_dir", dst_dir),
            ])
            .send()
            .await
            .unwrap()
    }

    // ========== Chunked Upload ==========

    /// POST a multipart form to an arbitrary URL (for upload-blks API calls).
    pub async fn post_multipart_url(
        &self,
        url: &str,
        form: reqwest::multipart::Form,
    ) -> reqwest::Response {
        self.client.post(url).multipart(form).send().await.unwrap()
    }

    /// GET /api2/repos/{repo_id}/upload-blks-link/
    pub async fn upload_blks_link(&self, token: &str, repo_id: &str) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/api2/repos/{}/upload-blks-link/",
                self.base_url, repo_id
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    /// GET /api2/repos/{repo_id}/update-blks-link/
    pub async fn update_blks_link(&self, token: &str, repo_id: &str) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/api2/repos/{}/update-blks-link/",
                self.base_url, repo_id
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    /// GET /api/v2.1/repos/{repo_id}/file-uploaded-bytes/
    pub async fn file_uploaded_bytes(
        &self,
        token: &str,
        repo_id: &str,
        file_name: &str,
        parent_dir: &str,
    ) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/api/v2.1/repos/{}/file-uploaded-bytes/?file_name={}&parent_dir={}",
                self.base_url, repo_id, file_name, parent_dir
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    // ========== Star feature helpers ==========

    /// POST /api/v2.1/starred-items/ with JSON body.
    pub async fn star_item(&self, token: &str, repo_id: &str, path: &str) -> reqwest::Response {
        self.post_json(
            "/api/v2.1/starred-items/",
            Some(token),
            &serde_json::json!({"repo_id": repo_id, "path": path}),
        )
        .await
    }

    /// DELETE /api/v2.1/starred-items/?repo_id=...&path=...
    pub async fn unstar_item(&self, token: &str, repo_id: &str, path: &str) -> reqwest::Response {
        // URL-encode the path for the query parameter
        let encoded: String = path
            .chars()
            .map(|c| match c {
                '/' => "%2F".to_string(),
                '?' => "%3F".to_string(),
                '#' => "%23".to_string(),
                _ => c.to_string(),
            })
            .collect();
        self.delete(
            &format!(
                "/api/v2.1/starred-items/?repo_id={}&path={}",
                repo_id, encoded
            ),
            Some(token),
        )
        .await
    }

    /// GET /api/v2.1/starred-items/
    pub async fn list_starred(&self, token: &str) -> reqwest::Response {
        self.get("/api/v2.1/starred-items/", Some(token)).await
    }

    // ========== Activity / operation helpers ==========

    /// GET /api/v2.1/activities/ with optional pagination.
    pub async fn get_activities(&self, token: &str, page: u32, per_page: u32) -> reqwest::Response {
        self.get(
            &format!("/api/v2.1/activities/?page={}&per_page={}", page, per_page),
            Some(token),
        )
        .await
    }

    /// POST /api2/repos/{repo_id}/file/rename/ with JSON body (v2 JSON endpoint).
    pub async fn rename_file(
        &self,
        token: &str,
        repo_id: &str,
        path: &str,
        new_name: &str,
    ) -> reqwest::Response {
        self.post_json(
            &format!("/api2/repos/{}/file/rename/", repo_id),
            Some(token),
            &serde_json::json!({"repo_id": repo_id, "p": path, "new_name": new_name}),
        )
        .await
    }

    /// POST /api2/repos/{src_repo}/file/move/ with JSON body (v2 JSON endpoint).
    pub async fn move_file(
        &self,
        token: &str,
        src_repo: &str,
        path: &str,
        _dst_repo: &str,
        dst_dir: &str,
    ) -> reqwest::Response {
        self.post_json(
            &format!("/api2/repos/{}/file/move/", src_repo),
            Some(token),
            &serde_json::json!({"repo_id": src_repo, "p": path, "new_parent_dir": dst_dir}),
        )
        .await
    }

    // ========== Encrypted Repo Operations ==========

    /// POST /api2/repos/ with client-side encryption params.
    pub async fn create_encrypted_repo(
        &self,
        token: &str,
        name: &str,
        repo_id: &str,
        magic: &str,
        random_key: &str,
        enc_version: i32,
    ) -> reqwest::Response {
        self.client
            .post(format!("{}/api2/repos/", self.base_url))
            .bearer_auth(token)
            .form(&[
                ("name", name),
                ("repo_id", repo_id),
                ("encrypted", "1"),
                ("enc_version", &enc_version.to_string()),
                ("magic", magic),
                ("random_key", random_key),
            ])
            .send()
            .await
            .unwrap()
    }

    /// POST /api2/repos/ with server-side password (enc_version 2).
    pub async fn create_encrypted_repo_with_password(
        &self,
        token: &str,
        name: &str,
        password: &str,
    ) -> reqwest::Response {
        self.client
            .post(format!("{}/api2/repos/", self.base_url))
            .bearer_auth(token)
            .form(&serde_json::json!({
                "name": name,
                "encrypted": 1,
                "enc_version": 2,
                "password": password,
            }))
            .send()
            .await
            .unwrap()
    }

    /// POST /api/v2.1/repos/{repo_id}/set-password/ with JSON body.
    pub async fn set_repo_password_v21(
        &self,
        token: &str,
        repo_id: &str,
        password: &str,
    ) -> reqwest::Response {
        self.post_json(
            &format!("/api/v2.1/repos/{repo_id}/set-password/"),
            Some(token),
            &serde_json::json!({"password": password}),
        )
        .await
    }

    /// POST /api2/repos/{repo_id}/?op=setpassword with form body.
    pub async fn set_repo_password_v2(
        &self,
        token: &str,
        repo_id: &str,
        password: &str,
    ) -> reqwest::Response {
        self.post_form(
            &format!("/api2/repos/{repo_id}/?op=setpassword"),
            Some(token),
            &[("password", password)],
        )
        .await
    }

    /// POST /api2/repos/{repo_id}/?op=checkpassword with magic form field.
    pub async fn check_repo_password_v2(
        &self,
        token: &str,
        repo_id: &str,
        magic: &str,
    ) -> reqwest::Response {
        self.post_form(
            &format!("/api2/repos/{repo_id}/?op=checkpassword"),
            Some(token),
            &[("magic", magic)],
        )
        .await
    }

    /// PUT /api/v2.1/repos/{repo_id}/set-password/?operation=change-password
    pub async fn change_repo_password(
        &self,
        token: &str,
        repo_id: &str,
        old_password: &str,
        new_password: &str,
    ) -> reqwest::Response {
        self.put_json(
            &format!("/api/v2.1/repos/{repo_id}/set-password/?operation=change-password"),
            Some(token),
            &serde_json::json!({
                "old_password": old_password,
                "new_password": new_password,
            }),
        )
        .await
    }
}
