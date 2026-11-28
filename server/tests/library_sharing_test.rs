//! Administrator-managed library sharing: visibility, user grants, the official
//! client's shapes, and the permission/revocation behaviour.

use crate::common;

use common::{TestFixture, TestServer, create_test_admin, create_test_user};
use serde_json::{Value, json};

struct SharingFixture {
    server: TestServer,
    admin_token: String,
    admin_id: i32,
    repo_id: String,
    user_token: String,
    user_email: String,
}

async fn login(client: &common::client::TestClient, email: &str, password: &str) -> String {
    let resp = client.login(email, password).await;
    assert_eq!(resp.status(), 200, "login failed for {email}");
    resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn setup() -> SharingFixture {
    let server = TestServer::start().await;
    let client = server.client();
    let admin_id = create_test_admin(&server.db, "admin@example.com", "password").await;
    let admin_token = login(&client, "admin@example.com", "password").await;
    let repo_id = common::create_test_repo(&client, &admin_token, "shared-lib").await;

    let user_id = create_test_user(&server.db, "reader@example.com", "password").await;
    let user_token = login(&client, "reader@example.com", "password").await;
    let _ = user_id;
    SharingFixture {
        server,
        admin_token,
        admin_id,
        repo_id,
        user_token,
        user_email: "reader@example.com".to_string(),
    }
}

async fn set_visibility(f: &SharingFixture, visibility: &str) -> reqwest::Response {
    f.server
        .client()
        .put_json(
            &format!("/api/repos/{}/visibility/", f.repo_id),
            Some(&f.admin_token),
            &json!({ "visibility": visibility }),
        )
        .await
}

async fn add_share(f: &SharingFixture, permission: &str) -> reqwest::Response {
    f.server
        .client()
        .put_json(
            &format!("/api2/repos/{}/dir/shared_items/?p=/", f.repo_id),
            Some(&f.admin_token),
            &json!({
                "share_type": "user",
                "username": f.user_email,
                "permission": permission,
            }),
        )
        .await
}

async fn member_token(f: &SharingFixture) -> String {
    let resp = f
        .server
        .client()
        .get(
            &format!("/api2/repos/{}/download-info/", f.repo_id),
            Some(&f.user_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "download-info failed");
    resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn public_library_is_a_grepo_for_every_account() {
    let f = setup().await;
    assert_eq!(set_visibility(&f, "public").await.status(), 200);

    let resp = f
        .server
        .client()
        .get("/api2/repos/", Some(&f.user_token))
        .await;
    assert_eq!(resp.status(), 200);
    let repos = resp.json::<Value>().await.unwrap();
    let repo = repos
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == f.repo_id)
        .expect("public library missing from the list");
    assert_eq!(repo["type"], "grepo");
    assert_eq!(repo["owner"], "Organization");
    assert_eq!(repo["permission"], "r");
    assert!(
        repo.get("groupid").is_none(),
        "a public library has no group id"
    );
    assert_eq!(repo["share_from"], "admin@example.com");

    let resp = f
        .server
        .client()
        .get("/api/v2.1/repos/", Some(&f.user_token))
        .await;
    let body = resp.json::<Value>().await.unwrap();
    let repo = body["repos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["repo_id"] == f.repo_id)
        .expect("public library missing from v2.1");
    assert_eq!(repo["type"], "public");
    assert_eq!(repo["permission"], "r");
}

#[tokio::test]
async fn public_reader_can_sync_but_cannot_write_or_create_links() {
    let f = setup().await;
    // A published library is not necessarily empty: seed one file so browsing and
    // downloading are exercised through the real read paths.
    let up = f
        .server
        .client()
        .upload_file(&f.admin_token, &f.repo_id, "/", "public.txt", b"public")
        .await;
    assert!(up.status().is_success(), "admin upload failed");
    assert_eq!(set_visibility(&f, "public").await.status(), 200);

    let token = member_token(&f).await;
    let client = f.server.client();

    // Browsing and downloading are the whole point of a public library: the listing
    // must answer 200 with `dir_perm: r` and the file bytes must be served.
    let resp = client.list_dir(&f.user_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200, "public reader must be able to list");
    assert_eq!(
        resp.headers().get("dir_perm").map(|v| v.to_str().unwrap()),
        Some("r"),
        "the desktop client's cloud file browser keys read-only mode off dir_perm"
    );
    let entries: Value = resp.json().await.unwrap();
    assert!(
        entries
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "public.txt"),
        "public reader must see the file: {entries}"
    );
    let resp = client
        .download_file(&f.user_token, &f.repo_id, "/public.txt")
        .await;
    assert_eq!(resp.status(), 200, "public reader must be able to download");
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"public");

    // The detail endpoint answers the published shape the clients classify as
    // "Shared with all" (a grepo owned by "Organization").
    let detail: Value = client
        .get_repo(&f.user_token, &f.repo_id)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(detail["type"], "grepo");
    assert_eq!(detail["owner"], "Organization");

    let resp = client
        .create_dir(&f.user_token, &f.repo_id, "/blocked")
        .await;
    assert_eq!(
        resp.status(),
        403,
        "a public reader must not create a folder"
    );

    let resp = client.get_sync("/seafhttp/accessible-repos", &token).await;
    assert_eq!(resp.status(), 200);
    let body = resp.json::<Value>().await.unwrap();
    let repo = body
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == f.repo_id)
        .expect("public library missing from accessible-repos");
    assert_eq!(repo["type"], "grepo");
    assert_eq!(repo["owner"], "Organization");

    let resp = client
        .get_sync(
            &format!("/seafhttp/repo/{}/permission-check/?op=download", f.repo_id),
            &token,
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "public reader must be allowed to download"
    );

    let resp = client
        .get_sync(
            &format!("/seafhttp/repo/{}/permission-check/?op=upload", f.repo_id),
            &token,
        )
        .await;
    assert_eq!(resp.status(), 403, "public reader must not upload");

    let resp = client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.user_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        403,
        "public reader must not mint an upload link"
    );

    let resp = client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.user_token),
            &json!({"repo_id": f.repo_id, "path": "/"}),
        )
        .await;
    assert_eq!(
        resp.status(),
        403,
        "public reader must not create anonymous share links"
    );
}

#[tokio::test]
async fn selected_user_is_an_srepo_with_the_granted_permission() {
    let f = setup().await;
    assert_eq!(add_share(&f, "r").await.status(), 200);

    let resp = f
        .server
        .client()
        .get("/api2/repos/", Some(&f.user_token))
        .await;
    let repos = resp.json::<Value>().await.unwrap();
    let repo = repos
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == f.repo_id)
        .expect("shared library missing");
    assert_eq!(repo["type"], "srepo");
    assert_eq!(repo["permission"], "r");
    assert_eq!(repo["owner"], "admin@example.com");

    let resp = f
        .server
        .client()
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.user_token),
        )
        .await;
    assert_eq!(resp.status(), 403, "read-only member must not upload");
}

#[tokio::test]
async fn read_write_member_can_upload() {
    let f = setup().await;
    assert_eq!(add_share(&f, "rw").await.status(), 200);

    let resp = f
        .server
        .client()
        .upload_file(&f.user_token, &f.repo_id, "/", "from-member.txt", b"member")
        .await;
    assert!(
        resp.status().is_success(),
        "rw member upload failed: {}",
        resp.status()
    );
}

#[tokio::test]
async fn share_management_shapes_match_the_desktop_client() {
    let f = setup().await;
    assert_eq!(add_share(&f, "rw").await.status(), 200);

    let client = f.server.client();
    let resp = client
        .get(
            &format!("/api2/repos/{}/dir/shared_items/?p=/", f.repo_id),
            Some(&f.admin_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.json::<Value>().await.unwrap();
    let items = body.as_array().expect("a bare array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["share_type"], "user");
    assert_eq!(items[0]["permission"], "rw");
    // The desktop client parses `user_info.name` as the username.
    assert_eq!(items[0]["user_info"]["name"], f.user_email);
    assert_eq!(items[0]["is_admin"], json!(false));

    // Change and remove the grant through the same surface.
    let resp = client
        .post_json(
            &format!(
                "/api2/repos/{}/dir/shared_items/?p=/&share_type=user&username={}",
                f.repo_id, f.user_email
            ),
            Some(&f.admin_token),
            &json!({"permission": "r"}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let resp = client
        .delete(
            &format!(
                "/api2/repos/{}/dir/shared_items/?p=/&share_type=user&username={}",
                f.repo_id, f.user_email
            ),
            Some(&f.admin_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .get(
            &format!("/api2/repos/{}/dir/shared_items/?p=/", f.repo_id),
            Some(&f.admin_token),
        )
        .await;
    let body = resp.json::<Value>().await.unwrap();
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn leaving_a_share_removes_access() {
    let f = setup().await;
    assert_eq!(add_share(&f, "r").await.status(), 200);
    let token = member_token(&f).await;

    let resp = f
        .server
        .client()
        .delete(
            &format!("/api2/beshared-repos/{}/?share_type=personal", f.repo_id),
            Some(&f.user_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        f.server
            .client()
            .get_sync(
                &format!("/seafhttp/repo/{}/permission-check/?op=download", f.repo_id),
                &token,
            )
            .await
            .status(),
        403
    );

    let resp = f
        .server
        .client()
        .get("/api2/repos/", Some(&f.user_token))
        .await;
    let repos = resp.json::<Value>().await.unwrap();
    assert!(
        repos
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["id"] != f.repo_id),
        "a left share must disappear from the list"
    );
}

#[tokio::test]
async fn switching_visibility_clears_the_other_audience() {
    let f = setup().await;
    assert_eq!(add_share(&f, "rw").await.status(), 200);
    assert_eq!(set_visibility(&f, "public").await.status(), 200);

    let resp = f
        .server
        .client()
        .get(
            &format!("/api2/repos/{}/dir/shared_items/?p=/", f.repo_id),
            Some(&f.admin_token),
        )
        .await;
    assert!(
        resp.json::<Value>()
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );

    assert_eq!(set_visibility(&f, "private").await.status(), 200);
    let resp = f
        .server
        .client()
        .get("/api2/repos/", Some(&f.user_token))
        .await;
    let repos = resp.json::<Value>().await.unwrap();
    assert!(
        repos
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["id"] != f.repo_id)
    );
}

#[tokio::test]
async fn only_an_administrator_owner_can_manage_sharing() {
    // A normal owner cannot use the share surface, even for their own library.
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/dir/shared_items/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 403);
    let resp = f
        .client
        .put_json(
            &format!("/api/repos/{}/visibility/", f.repo_id),
            Some(&f.api_token),
            &json!({"visibility": "public"}),
        )
        .await;
    assert_eq!(resp.status(), 403);

    // An administrator who does not own the library cannot manage it either.
    let s = setup().await;
    let other_admin = create_test_admin(&s.server.db, "admin2@example.com", "password").await;
    assert_ne!(other_admin, s.admin_id);
    let other_token = login(&s.server.client(), "admin2@example.com", "password").await;
    let resp = s
        .server
        .client()
        .put_json(
            &format!("/api/repos/{}/visibility/", s.repo_id),
            Some(&other_token),
            &json!({"visibility": "public"}),
        )
        .await;
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn encrypted_libraries_cannot_be_shared_or_published() {
    let f = setup().await;
    let resp = f
        .server
        .client()
        .create_repo_multipart_with_passwd(&f.admin_token, "encrypted-lib", "secret")
        .await;
    assert_eq!(resp.status(), 200);
    let repo_id = resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = f
        .server
        .client()
        .put_json(
            &format!("/api/repos/{repo_id}/visibility/"),
            Some(&f.admin_token),
            &json!({"visibility": "public"}),
        )
        .await;
    assert_eq!(resp.status(), 400);
    let resp = f
        .server
        .client()
        .put_json(
            &format!("/api2/repos/{repo_id}/dir/shared_items/?p=/"),
            Some(&f.admin_token),
            &json!({"share_type": "user", "username": f.user_email, "permission": "r"}),
        )
        .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn folder_paths_are_rejected() {
    let f = setup().await;
    let client = f.server.client();
    assert_eq!(
        client
            .create_dir(&f.admin_token, &f.repo_id, "/docs")
            .await
            .status()
            .as_u16(),
        201
    );
    let resp = client
        .put_json(
            &format!("/api2/repos/{}/dir/shared_items/?p=/docs", f.repo_id),
            Some(&f.admin_token),
            &json!({"share_type": "user", "username": f.user_email, "permission": "r"}),
        )
        .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn related_users_lists_the_owner_and_members_but_not_public_readers() {
    let f = setup().await;
    assert_eq!(add_share(&f, "r").await.status(), 200);

    let resp = f
        .server
        .client()
        .get(
            &format!("/api/v2.1/repos/{}/related-users/", f.repo_id),
            Some(&f.admin_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.json::<Value>().await.unwrap();
    let emails: Vec<&str> = body["user_list"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|u| u["email"].as_str())
        .collect();
    assert!(emails.contains(&"admin@example.com"));
    assert!(emails.contains(&"reader@example.com"));

    assert_eq!(set_visibility(&f, "public").await.status(), 200);
    let resp = f
        .server
        .client()
        .get(
            &format!("/api/v2.1/repos/{}/related-users/", f.repo_id),
            Some(&f.admin_token),
        )
        .await;
    let body = resp.json::<Value>().await.unwrap();
    assert_eq!(
        body["user_list"].as_array().unwrap().len(),
        1,
        "public access must not turn every account into a related user"
    );
}

#[tokio::test]
async fn administrator_cannot_demote_itself_while_owning_a_shared_library() {
    let f = setup().await;
    assert_eq!(set_visibility(&f, "public").await.status(), 200);

    let resp = f
        .server
        .client()
        .put_json(
            &format!("/api/admin/users/{}/", f.admin_id),
            Some(&f.admin_token),
            &json!({"is_admin": false, "is_active": true}),
        )
        .await;
    assert_eq!(resp.status(), 400);

    assert_eq!(set_visibility(&f, "private").await.status(), 200);
    let resp = f
        .server
        .client()
        .put_json(
            &format!("/api/admin/users/{}/", f.admin_id),
            Some(&f.admin_token),
            &json!({"is_admin": false, "is_active": true}),
        )
        .await;
    // The test's admin token may lose its admin privileges after this succeeds;
    // the assertion is only about the guard above.
    assert!(matches!(resp.status().as_u16(), 200 | 403));
}

#[tokio::test]
async fn user_search_is_administrator_only_and_capped() {
    let f = setup().await;
    let client = f.server.client();
    let resp = client
        .get("/api2/search-user/?q=reader", Some(&f.user_token))
        .await;
    assert_eq!(resp.status(), 403);

    let resp = client
        .get("/api2/search-user/?q=reader", Some(&f.admin_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.json::<Value>().await.unwrap();
    assert_eq!(body["users"][0]["email"], "reader@example.com");
    assert_eq!(body["users"][0]["name"], "reader");
}

/// One `/api2/repos/` entry by id.
fn api2_repo<'a>(repos: &'a Value, repo_id: &str) -> &'a Value {
    repos
        .as_array()
        .unwrap_or_else(|| panic!("expected an array, got {repos}"))
        .iter()
        .find(|r| r["id"] == repo_id)
        .unwrap_or_else(|| panic!("library {repo_id} missing from {repos}"))
}

/// One `/api/v2.1/repos/` entry by id.
fn v21_repo<'a>(body: &'a Value, repo_id: &str) -> Option<&'a Value> {
    body["repos"]
        .as_array()?
        .iter()
        .find(|r| r["repo_id"] == repo_id)
}

/// Fields the desktop client reads unconditionally.
///
/// `ServerRepo::fromJSON` (`seafile-client/client/src/api/server-repo.cpp:17-50`)
/// reads these keys for every entry; a missing key silently becomes the empty
/// string/zero, so an entry it cannot classify is worse than a failed request.
fn assert_desktop_parses(repo: &Value) {
    for key in [
        "id",
        "name",
        "type",
        "permission",
        "mtime",
        "size",
        "encrypted",
        "virtual",
        "version",
    ] {
        assert!(
            repo.get(key).is_some(),
            "the desktop client reads `{key}`: {repo}"
        );
    }
    let head = repo["head_commit_id"]
        .as_str()
        .unwrap_or_else(|| panic!("head_commit_id must be a string: {repo}"));
    assert_eq!(head.len(), 40, "head_commit_id must be 40 hex: {repo}");
    assert!(
        head.chars().all(|c| c.is_ascii_hexdigit()),
        "head_commit_id must be 40 hex: {repo}"
    );
    // SeaDrive's `parse_repo_list()` skips entries whose version is 0, which would
    // silently hide the library from the mounted drive.
    assert!(
        repo["version"].as_i64().unwrap_or(0) >= 1,
        "version must be non-zero: {repo}"
    );
}

/// The desktop client's category decision for one entry.
///
/// `RepoTreeModel::setRepos()` (`seafile-client/client/src/ui/repo-tree-model.cpp:160-170`)
/// sends `repo` to "My Libraries", `srepo` to "Shared with me", `grepo` with
/// `group_id == 0` to "Shared with all" and any other `grepo` to a group category.
/// `ServerRepo::fromJSON` maps the JSON `owner` to `group_name` and `groupid` to
/// `group_id` for `grepo`, so an absent `groupid` reads as 0 (`json_integer_value(NULL)`).
/// Returns (category, readonly).
fn desktop_view(repo: &Value) -> (&'static str, bool) {
    let repo_type = repo["type"].as_str().unwrap_or_default();
    let readonly = repo["permission"].as_str() != Some("rw");
    match repo_type {
        "repo" => ("my libraries", readonly),
        "srepo" => ("shared with me", readonly),
        "grepo" => {
            let group_id = repo.get("groupid").and_then(Value::as_i64).unwrap_or(0);
            if group_id == 0 {
                ("shared with all", readonly)
            } else {
                ("shared with groups", readonly)
            }
        }
        // No type → the client falls back to `checkGroupRepo()`, an empty-named
        // group category. Treat that as a failure in the assertions below.
        _ => ("unclassified", readonly),
    }
}

/// How SeaDrive's `parse_repo_list()` (`seadrive-fuse/src/sync-mgr.c:1294-1435`)
/// classifies one entry. It rejects the whole response when a required member is
/// missing or malformed, so the presence/format assertions are part of the contract.
fn seadrive_view(repo: &Value) -> &'static str {
    assert_desktop_parses(repo);
    let repo_type = repo["type"].as_str().expect("type");
    let owner = repo["owner"].as_str().expect("owner");
    assert_eq!(
        repo["id"].as_str().map(str::len),
        Some(36),
        "SeaDrive requires a UUID: {repo}"
    );
    match repo_type {
        "repo" => "mine",
        "srepo" => "shared",
        "grepo" if owner == "Organization" => "public",
        "grepo" => "group",
        other => panic!("SeaDrive rejects type {other:?}: {repo}"),
    }
}

/// What Android and iOS need from one `/api/v2.1/repos/` entry.
///
/// Both clients request that endpoint for the library list
/// (`seadroid/.../ui/repo/RepoService.java:25` and every `SeafRepos.handleData:` fetch
/// in `seafile-iOS/Pod/Classes/SeafRepos.m`), require `repo_id`/`repo_name` strings,
/// and group by `type` (`RepoType.TYPE_MINE/TYPE_SHARED/TYPE_PUBLIC`; iOS'
/// `MINE_REPO`/`SHARE_REPO`/`PUBLIC_REPO`). Only `type == "group"` entries may carry
/// `group_id`/`group_name`.
fn assert_mobile_parses(repo: &Value, expected_type: &str) {
    for key in [
        "repo_id",
        "repo_name",
        "type",
        "permission",
        "size",
        "encrypted",
        "last_modified",
        "owner_email",
        "owner_name",
    ] {
        assert!(
            repo.get(key).is_some(),
            "the mobile clients read `{key}`: {repo}"
        );
    }
    assert_eq!(repo["type"], expected_type);
    assert!(
        repo["repo_name"].is_string(),
        "iOS rejects the whole list when repo_name is not a string: {repo}"
    );
    assert!(
        repo.get("group_id").is_none() && repo.get("group_name").is_none(),
        "only a group-type entry may carry group fields: {repo}"
    );
}

async fn update_share(f: &SharingFixture, permission: &str) -> reqwest::Response {
    f.server
        .client()
        .post_json(
            &format!(
                "/api2/repos/{}/dir/shared_items/?p=/&share_type=user&username={}",
                f.repo_id, f.user_email
            ),
            Some(&f.admin_token),
            &json!({ "permission": permission }),
        )
        .await
}

/// The official clients must see each sharing state exactly as a real seafile does:
/// the desktop client's four categories, SeaDrive's parse rules, and the v2.1 type
/// vocabulary the mobile clients group by.
#[tokio::test]
async fn official_clients_classify_the_sharing_states_like_upstream() {
    let f = setup().await;
    let client = f.server.client();

    // ---- selected user, read-only -------------------------------------------------
    assert_eq!(add_share(&f, "r").await.status(), 200);
    let repos: Value = client
        .get("/api2/repos/", Some(&f.user_token))
        .await
        .json()
        .await
        .unwrap();
    let repo = api2_repo(&repos, &f.repo_id);
    assert_eq!(desktop_view(repo), ("shared with me", true));
    assert_eq!(repo["type"], "srepo");
    assert_eq!(repo["owner"], "admin@example.com");
    assert_eq!(repo["owner_name"], "admin");
    assert!(
        repo.get("share_from").is_none(),
        "a personal share is an `srepo`, not a published library: {repo}"
    );
    assert_eq!(seadrive_view(repo), "shared");

    let body: Value = client
        .get("/api/v2.1/repos/", Some(&f.user_token))
        .await
        .json()
        .await
        .unwrap();
    assert_mobile_parses(v21_repo(&body, &f.repo_id).unwrap(), "shared");
    assert_eq!(v21_repo(&body, &f.repo_id).unwrap()["permission"], "r");

    // ---- the same user upgraded to read-write -------------------------------------
    assert_eq!(update_share(&f, "rw").await.status(), 200);
    let repos: Value = client
        .get("/api2/repos/", Some(&f.user_token))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        desktop_view(api2_repo(&repos, &f.repo_id)),
        ("shared with me", false)
    );

    // ---- published to every account ------------------------------------------------
    assert_eq!(set_visibility(&f, "public").await.status(), 200);
    let repos: Value = client
        .get("/api2/repos/", Some(&f.user_token))
        .await
        .json()
        .await
        .unwrap();
    let repo = api2_repo(&repos, &f.repo_id);
    assert_eq!(desktop_view(repo), ("shared with all", true));
    assert_eq!(repo["type"], "grepo");
    assert_eq!(repo["owner"], "Organization");
    assert_eq!(repo["share_from"], "admin@example.com");
    assert_eq!(repo["share_from_name"], "admin");
    assert!(
        repo.get("groupid").is_none(),
        "`grepo` with a group id would land in a group category instead of \
         \"Shared with all\": {repo}"
    );
    assert_eq!(seadrive_view(repo), "public");

    let body: Value = client
        .get("/api/v2.1/repos/", Some(&f.user_token))
        .await
        .json()
        .await
        .unwrap();
    assert_mobile_parses(v21_repo(&body, &f.repo_id).unwrap(), "public");

    // The owner's own listing keeps the library in "My Libraries" (type `repo`,
    // owner permission `rw`), matching seahub's `mine` block.
    let repos: Value = client
        .get("/api2/repos/", Some(&f.admin_token))
        .await
        .json()
        .await
        .unwrap();
    let repo = api2_repo(&repos, &f.repo_id);
    assert_eq!(desktop_view(repo), ("my libraries", false));
    assert_eq!(repo["type"], "repo");
    assert_eq!(repo["permission"], "rw");
    assert_eq!(seadrive_view(repo), "mine");

    let body: Value = client
        .get("/api/v2.1/repos/", Some(&f.admin_token))
        .await
        .json()
        .await
        .unwrap();
    assert_mobile_parses(v21_repo(&body, &f.repo_id).unwrap(), "mine");

    // ---- restricted again: every client's list must drop it ------------------------
    assert_eq!(set_visibility(&f, "private").await.status(), 200);
    let repos: Value = client
        .get("/api2/repos/", Some(&f.user_token))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        !repos
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == f.repo_id),
        "a private library must disappear from the recipient's list: {repos}"
    );
}

/// The desktop client's share dialog sends the exact encodings below; the endpoints
/// must accept them (not just the JSON forms the web UI uses).
///
/// Sources: `PrivateShareRequest` (`seafile-client/src/api/requests.cpp:1037-1083`)
/// puts `permission`/`share_type`/`username` in the form on PUT and in the query on
/// POST/DELETE, always with `p` in the query; `GetPrivateShareItemsRequest`
/// (`requests.cpp:1170-1235`) parses a bare array whose `user_info.name` is the email;
/// `SearchUsersRequest` reads `{"users": […]}`; `FetchGroupsRequest` reads a top-level
/// array. The recipient side is Android's `deleteRepoShare`
/// (`seadroid/.../ui/repo/RepoService.java:56-57`), which adds `from=<owner>`.
#[tokio::test]
async fn desktop_share_dialog_encodings_are_accepted() {
    let f = setup().await;
    let client = f.server.client();
    let shared_items = format!("/api2/repos/{}/dir/shared_items/", f.repo_id);
    let with_p = format!("{shared_items}?p=/");

    // Opening the dialog lists the shares (bare array).
    let resp = client.get(&with_p, Some(&f.admin_token)).await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.json::<Value>()
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Adding a share: everything but `p` arrives as form fields.
    let resp = client
        .put_form(
            &with_p,
            Some(&f.admin_token),
            &[
                ("permission", "rw"),
                ("share_type", "user"),
                ("username", f.user_email.as_str()),
            ],
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "the desktop client's PUT failed: {:?}",
        resp.text().await
    );
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["failed"], json!([]));
    assert_eq!(body["success"][0]["user_info"]["name"], f.user_email);
    assert_eq!(body["success"][0]["user_info"]["nickname"], "reader");
    assert_eq!(body["success"][0]["permission"], "rw");

    // Re-listing returns what the dialog's model parses.
    let items: Value = client
        .get(&with_p, Some(&f.admin_token))
        .await
        .json()
        .await
        .unwrap();
    let items = items.as_array().unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["share_type"], "user");
    assert_eq!(items[0]["user_info"]["name"], f.user_email);
    assert_eq!(items[0]["permission"], "rw");

    // Changing the permission: `p`/`share_type`/`username` in the query, permission
    // in the form.
    let resp = client
        .post_form(
            &format!(
                "{shared_items}?p=/&share_type=user&username={}",
                f.user_email
            ),
            Some(&f.admin_token),
            &[("permission", "r")],
        )
        .await;
    assert_eq!(resp.status(), 200, "the desktop client's POST failed");
    let items: Value = client
        .get(&with_p, Some(&f.admin_token))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(items[0]["permission"], "r");

    // The dialog's completer endpoints.
    let users: Value = client
        .get("/api2/search-user/?q=reader", Some(&f.admin_token))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(users["users"][0]["email"], f.user_email);
    assert!(users["users"][0].get("contact_email").is_some());
    let groups: Value = client
        .get("/api2/groups/?with_msg=false", Some(&f.admin_token))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        groups.as_array().is_some(),
        "the group completer parses a bare array: {groups}"
    );

    // Removing the share: every parameter is in the query.
    let resp = client
        .delete(
            &format!(
                "{shared_items}?p=/&share_type=user&username={}",
                f.user_email
            ),
            Some(&f.admin_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "the desktop client's DELETE failed");
    let items: Value = client
        .get(&with_p, Some(&f.admin_token))
        .await
        .json()
        .await
        .unwrap();
    assert!(items.as_array().unwrap().is_empty());

    // Android's "leave share" call carries the owner in `from`, which the server
    // ignores (it derives the owner from the row).
    assert_eq!(add_share(&f, "r").await.status(), 200);
    let resp = client
        .delete(
            &format!(
                "/api2/beshared-repos/{}/?share_type=personal&from=admin@example.com",
                f.repo_id
            ),
            Some(&f.user_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "Android's leave-share DELETE failed");
    let repos: Value = client
        .get("/api2/repos/", Some(&f.user_token))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        !repos
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == f.repo_id),
        "a left share must disappear from the list: {repos}"
    );
}
