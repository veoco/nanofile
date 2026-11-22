//! The repository surface contract: the shapes v1, v2.1, sync and the web UI
//! expose before the shared loader/serializer refactor.
//!
//! These tests pin the fields official clients actually read (desktop
//! `ServerRepo::fromJSON`, Android `RepoModel`/`RepoInfoModel`) and the
//! currently-documented divergences, so a dedup refactor cannot change them
//! silently.

use crate::common;

use common::TestFixture;
use common::client::EncRepoCreate;
use common::{add_repo_member, create_test_repo, create_test_user};
use infra::crypto::key_derivation;
use serde_json::Value;

async fn upload(f: &TestFixture, name: &str) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", name, b"repo surface")
        .await;
    assert!(resp.status().is_success(), "upload failed");
}

async fn member_token(f: &TestFixture, email: &str, permission: &str) -> (String, i32) {
    let user_id = create_test_user(&f.server.db, email, "password").await;
    add_repo_member(&f.server.db, &f.repo_id, user_id, permission).await;
    let resp = f.client.login(email, "password").await;
    assert_eq!(resp.status(), 200, "member login failed");
    let body: Value = resp.json().await.unwrap();
    (body["token"].as_str().unwrap().to_string(), user_id)
}

fn assert_keys(value: &Value, keys: &[&str]) {
    for key in keys {
        assert!(value.get(*key).is_some(), "missing {key} in {value}");
    }
}

/// Desktop reads exactly this subset from `GET /api2/repos/`.
#[tokio::test]
async fn test_v1_repo_list_owned_fields() {
    let f = TestFixture::new().await;
    upload(&f, "a.txt").await;

    let resp = f.client.list_repos(&f.api_token).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let repos = body.as_array().expect("v1 list is an array");
    assert_eq!(repos.len(), 1);
    let repo = &repos[0];

    assert_keys(
        repo,
        &[
            "id",
            "name",
            "desc",
            "mtime",
            "size",
            "encrypted",
            "permission",
            "head_commit_id",
            "version",
            "type",
            "virtual",
            "root",
            "owner",
            "owner_name",
        ],
    );
    assert_eq!(repo["id"], f.repo_id);
    assert_eq!(repo["type"], "repo");
    assert_eq!(repo["permission"], "rw");
    assert_eq!(repo["encrypted"], serde_json::json!(false));
    assert_eq!(repo["virtual"], serde_json::json!(false));
    assert_eq!(repo["owner"], f.email);
    assert!(repo["size"].as_i64().unwrap() > 0);
    assert!(repo["mtime"].as_i64().unwrap() > 0);
    assert_eq!(repo["head_commit_id"].as_str().map(str::len), Some(40));
    assert!(repo["owner_name"].as_str().is_some_and(|s| !s.is_empty()));
}

/// The v1 single-repo answer: Android parses it (`RepoInfoModel`), and its
/// current `owner`/`type` values are a documented divergence from seahub.
#[tokio::test]
async fn test_v1_repo_get_fields() {
    let f = TestFixture::new().await;
    upload(&f, "a.txt").await;

    let resp = f.client.get_repo(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let repo: Value = resp.json().await.unwrap();
    assert_keys(
        &repo,
        &[
            "id",
            "name",
            "type",
            "owner",
            "mtime",
            "size",
            "encrypted",
            "root",
            "permission",
            "head_commit_id",
        ],
    );
    assert_eq!(repo["type"], "repo");
    assert_eq!(repo["owner"], f.email);
    assert_eq!(repo["root"].as_str().map(str::len), Some(40));
    assert_eq!(repo["head_commit_id"].as_str().map(str::len), Some(40));
}

/// A shared member sees the library in both v1 endpoints with the membership
/// permission; the owner fields carry the owner's identity.
#[tokio::test]
async fn test_v1_repo_shared_member_fields() {
    let f = TestFixture::new().await;
    upload(&f, "a.txt").await;
    let (token, _) = member_token(&f, "reader@test.com", "r").await;

    let resp = f.client.list_repos(&token).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let repo = &body.as_array().unwrap()[0];
    assert_eq!(repo["permission"], "r");
    assert_eq!(repo["type"], "srepo");
    assert_eq!(repo["owner"], f.email);
    assert!(repo.get("owner_name").is_some());

    let resp = f.client.get_repo(&token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let repo: Value = resp.json().await.unwrap();
    assert_eq!(repo["permission"], "r");
    // Current implementation answers `srepo` here; seahub always answers
    // `repo` and uses owner="share". Documented divergence, pinned.
    assert_eq!(repo["type"], "srepo");
}

/// Android parses the v2.1 list with `RepoModel`: these fields must exist and
/// `last_modified` must parse with its `yyyy-MM-dd'T'HH:mm:ssXXX` format.
#[tokio::test]
async fn test_v21_repo_list_and_get_fields() {
    let f = TestFixture::new().await;
    upload(&f, "a.txt").await;

    let resp = f.client.get("/api/v2.1/repos/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let repos = body["repos"].as_array().expect("v2.1 envelope");
    assert_eq!(repos.len(), 1);
    let repo = &repos[0];
    assert_keys(
        repo,
        &[
            "repo_id",
            "repo_name",
            "repo_desc",
            "permission",
            "encrypted",
            "type",
            "size",
            "last_modified",
            "mtime",
            "owner_email",
            "owner_name",
            "owner_contact_email",
        ],
    );
    assert_eq!(repo["repo_id"], f.repo_id);
    assert_eq!(repo["type"], "mine");
    assert_eq!(repo["permission"], "rw");
    assert_eq!(repo["encrypted"], serde_json::json!(false));
    assert_eq!(repo["owner_email"], f.email);
    let last_modified = repo["last_modified"].as_str().unwrap();
    chrono::DateTime::parse_from_rfc3339(last_modified)
        .expect("last_modified must be RFC 3339 for Android's parser");
    assert!(
        repo.get("salt").is_none(),
        "an unencrypted library has no salt"
    );

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let single: Value = resp.json().await.unwrap();
    assert_keys(
        &single,
        &[
            "repo_id",
            "repo_name",
            "permission",
            "encrypted",
            "type",
            "size",
            "last_modified",
            "owner_email",
            "owner_name",
        ],
    );
    assert_eq!(single["type"], "mine");
}

#[tokio::test]
async fn test_v21_repo_shared_member_type() {
    let f = TestFixture::new().await;
    upload(&f, "a.txt").await;
    let (token, _) = member_token(&f, "reader@test.com", "r").await;

    let resp = f.client.get("/api/v2.1/repos/", Some(&token)).await;
    let body: Value = resp.json().await.unwrap();
    let repo = &body["repos"].as_array().unwrap()[0];
    assert_eq!(repo["type"], "shared");
    assert_eq!(repo["permission"], "r");
    assert_eq!(repo["owner_email"], f.email);

    let resp = f
        .client
        .get(&format!("/api/v2.1/repos/{}/", f.repo_id), Some(&token))
        .await;
    let single: Value = resp.json().await.unwrap();
    assert_eq!(single["type"], "shared");
    assert_eq!(single["permission"], "r");
}

/// SeaDrive parses this response with `parse_repo_list()`, which requires
/// seafile-server's `SharedRepo` members.
#[tokio::test]
async fn test_sync_accessible_repos_fields() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get_sync("/seafhttp/accessible-repos", &f.sync_token)
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let repo = &body.as_array().unwrap()[0];
    assert_keys(
        repo,
        &[
            "version",
            "id",
            "head_commit_id",
            "name",
            "mtime",
            "permission",
            "type",
            "owner",
        ],
    );
    assert_eq!(repo["permission"], "rw");
    assert_eq!(repo["type"], "repo");
    assert_eq!(repo["owner"], f.email.to_lowercase());
    assert_eq!(
        repo["head_commit_id"].as_str(),
        Some("0000000000000000000000000000000000000000"),
        "a headless library keeps the all-zero sentinel"
    );
}

/// Android stores `RepoModel.salt` from the v2.1 list; a v4 library must carry
/// the per-library salt it was created with (seahub sends it for enc_version>=3).
#[tokio::test]
async fn test_v21_encrypted_repo_exposes_salt() {
    let f = TestFixture::new().await;
    let repo_id = uuid::Uuid::new_v4().to_string();
    let salt = key_derivation::generate_repo_salt();
    let magic = key_derivation::generate_magic(&repo_id, "pw", 4, &salt).unwrap();
    let random_key = key_derivation::generate_random_key_for_repo("pw", 4, &salt).unwrap();
    let resp = f
        .client
        .create_encrypted_repo_with_params(
            &f.api_token,
            &EncRepoCreate {
                name: "v4-lib",
                repo_id: &repo_id,
                magic: &magic,
                random_key: &random_key,
                enc_version: 4,
                salt: Some(&salt),
            },
        )
        .await;
    assert_eq!(resp.status(), 200, "create v4 library failed");

    let body: Value = f
        .client
        .get("/api/v2.1/repos/", Some(&f.api_token))
        .await
        .json()
        .await
        .unwrap();
    let repo = body["repos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["repo_id"] == repo_id)
        .expect("created library missing from the list");
    assert_eq!(repo["encrypted"], serde_json::json!(true));
    assert_eq!(repo["salt"], serde_json::json!(salt));
}

/// The web UI lists both owned and shared libraries, from one page.
#[tokio::test]
async fn test_ui_libraries_lists_owned_and_shared() {
    let f = TestFixture::new().await;

    // The member owns `mine` and is a member of the fixture's `test-repo`.
    let (member_api_token, user_id) = member_token(&f, "reader@test.com", "rw").await;
    let _ = create_test_repo(&f.client, &member_api_token, "member-repo").await;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();
    let resp = client
        .post(format!("{}/accounts/login/", f.server.base_url))
        .form(&[
            ("email", "reader@test.com".to_string()),
            ("password", "password".to_string()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "web login failed");
    let resp = client
        .get(format!("{}/libraries/", f.server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.unwrap();
    assert!(html.contains("test-repo"), "shared library missing");
    assert!(html.contains("member-repo"), "owned library missing");
    let _ = user_id;
}
