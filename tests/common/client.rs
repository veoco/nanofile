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
        self.client
            .get(format!(
                "{}/api2/repos/{}/file/?p={}",
                self.base_url, repo_id, path
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    pub async fn list_dir(&self, token: &str, repo_id: &str, path: &str) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/api2/repos/{}/dir/?p={}",
                self.base_url, repo_id, path
            ))
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
}
