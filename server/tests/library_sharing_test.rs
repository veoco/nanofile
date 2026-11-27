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
            &format!("/api/v2.1/repos/{}/visibility/", f.repo_id),
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
    assert_eq!(set_visibility(&f, "public").await.status(), 200);

    let token = member_token(&f).await;
    let client = f.server.client();

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
            &format!("/api/v2.1/repos/{}/visibility/", f.repo_id),
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
            &format!("/api/v2.1/repos/{}/visibility/", s.repo_id),
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
            &format!("/api/v2.1/repos/{repo_id}/visibility/"),
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
            &format!("/api2/admin/users/{}/", f.admin_id),
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
            &format!("/api2/admin/users/{}/", f.admin_id),
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
