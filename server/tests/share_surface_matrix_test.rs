//! The share-link and upload-link wire shapes, pinned before the JSON
//! assemblers move into the sharing services.
//!
//! The field sets below are what the web SPA, desktop and Android currently
//! parse; several already differ from seahub and are documented in
//! `docs/reference/api/compatibility.md`.

use crate::common;

use common::TestFixture;
use serde_json::{Value, json};

async fn upload(f: &TestFixture, dir: &str, name: &str) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, dir, name, b"link payload")
        .await;
    assert!(resp.status().is_success(), "upload failed");
}

fn assert_keys(value: &Value, keys: &[&str]) {
    for key in keys {
        assert!(value.get(*key).is_some(), "missing {key} in {value}");
    }
}

async fn create_share(f: &TestFixture, path: &str) -> Value {
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &json!({ "repo_id": f.repo_id, "path": path }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    resp.json().await.unwrap()
}

/// POST/GET v2.1 share-links: the canonical create field set, and the
/// slightly narrower GET/build set (current implementation).
#[tokio::test]
async fn test_v21_share_link_create_and_get_fields() {
    let f = TestFixture::new().await;
    upload(&f, "/", "s.txt").await;

    let created = create_share(&f, "/s.txt").await;
    assert_keys(
        &created,
        &[
            "token",
            "link",
            "repo_id",
            "repo_name",
            "path",
            "obj_name",
            "s_type",
            "is_dir",
            "username",
            "view_cnt",
            "ctime",
            "expire_date",
            "is_expired",
            "has_password",
            "permissions",
            "password",
            "description",
        ],
    );
    assert_eq!(created["repo_id"], f.repo_id);
    assert_eq!(created["path"], "/s.txt");
    assert_eq!(created["obj_name"], "s.txt");
    assert_eq!(created["s_type"], "f");
    assert_eq!(created["is_dir"], json!(false));
    assert_eq!(created["has_password"], json!(false));
    assert_eq!(created["view_cnt"], json!(0));
    assert_eq!(created["is_expired"], json!(false));
    assert!(created["ctime"].is_number(), "ctime is pinned as epoch");
    assert!(created["link"].as_str().unwrap().contains("/f/"));
    assert_eq!(
        created["permissions"],
        json!({ "can_edit": false, "can_download": true, "can_upload": false })
    );

    let token = created["token"].as_str().unwrap().to_string();
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/share-links/{token}/"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let got: Value = resp.json().await.unwrap();
    assert_keys(
        &got,
        &[
            "token",
            "link",
            "repo_id",
            "repo_name",
            "path",
            "obj_name",
            "is_dir",
            "username",
            "view_cnt",
            "ctime",
            "expire_date",
            "is_expired",
            "has_password",
            "permissions",
            "password",
            "description",
        ],
    );
    assert_eq!(got["token"], token);
    assert_eq!(got["obj_name"], "s.txt");
    assert_eq!(got["is_dir"], json!(false));
}

/// A folder share keeps `s_type=d`, `is_dir=true` and the `/d/` page link.
#[tokio::test]
async fn test_v21_share_link_dir_shape() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/shared-dir")
        .await;
    assert!(resp.status().is_success(), "mkdir failed");

    let created = create_share(&f, "/shared-dir").await;
    assert_eq!(created["s_type"], "d");
    assert_eq!(created["is_dir"], json!(true));
    assert_eq!(created["obj_name"], "shared-dir");
    assert!(created["link"].as_str().unwrap().contains("/d/"));
}

/// The list endpoint carries the legacy list fields (`created_at`,
/// `expire_at`), which differ from the create/get set.
#[tokio::test]
async fn test_v21_share_link_list_shape() {
    let f = TestFixture::new().await;
    upload(&f, "/", "s.txt").await;
    let created = create_share(&f, "/s.txt").await;
    let token = created["token"].as_str().unwrap();

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/share-links/?repo_id={}&path=/s.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let items: Value = resp.json().await.unwrap();
    let item = &items.as_array().unwrap()[0];
    assert_keys(
        item,
        &[
            "token",
            "link",
            "repo_id",
            "path",
            "created_at",
            "has_password",
            "expire_at",
            "s_type",
            "view_cnt",
            "description",
        ],
    );
    assert_eq!(item["token"], token);
    assert_eq!(item["s_type"], "f");
}

/// `multi-share-links` is currently the same shape as a single create.
#[tokio::test]
async fn test_v21_multi_share_link_shape() {
    let f = TestFixture::new().await;
    upload(&f, "/", "s.txt").await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/multi-share-links/",
            Some(&f.api_token),
            &json!({ "repo_id": f.repo_id, "path": "/s.txt" }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_keys(
        &body,
        &["token", "link", "repo_id", "path", "obj_name", "is_dir"],
    );
    assert_eq!(body["obj_name"], "s.txt");
    assert_eq!(body["is_dir"], json!(false));
}

/// The v1 typed share-link answer used by older clients.
#[tokio::test]
async fn test_v1_shared_links_shape() {
    let f = TestFixture::new().await;
    upload(&f, "/", "s.txt").await;

    let resp = f
        .client
        .post_json(
            "/api2/shared-links",
            Some(&f.api_token),
            &json!({ "repo_id": f.repo_id, "path": "/s.txt" }),
        )
        .await;
    let status = resp.status();
    let text = resp.text().await.unwrap();
    assert_eq!(status, 200, "v1 create shared-links body: {text}");
    let created: Value = serde_json::from_str(&text).unwrap();
    assert_keys(
        &created,
        &[
            "token",
            "link",
            "repo_id",
            "path",
            "created_at",
            "has_password",
            "expire_at",
            "s_type",
            "view_cnt",
            "description",
        ],
    );
    assert_eq!(created["s_type"], "f");
    assert!(created["link"].as_str().unwrap().contains("/f/"));

    let resp = f.client.get("/api2/shared-links", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let items: Value = resp.json().await.unwrap();
    assert_keys(&items.as_array().unwrap()[0], &["token", "link", "s_type"]);
}

/// POST v2.1 upload-links and its GET/list variants; the `link` prefix is
/// `/u/{token}/` and is deliberately kept as-is.
#[tokio::test]
async fn test_v21_upload_link_shapes() {
    let f = TestFixture::new().await;
    let site = f.server.base_url.trim_end_matches('/');

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &json!({ "repo_id": f.repo_id, "path": "/" }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let created: Value = resp.json().await.unwrap();
    assert_keys(
        &created,
        &[
            "token",
            "link",
            "repo_id",
            "path",
            "ctime",
            "username",
            "expire_date",
            "is_expired",
            "has_password",
            "password",
            "description",
            "view_cnt",
            "obj_name",
        ],
    );
    let token = created["token"].as_str().unwrap().to_string();
    let link = created["link"].as_str().unwrap();
    assert!(
        link.starts_with(&format!("{site}/u/")),
        "upload link must keep the current /u/{{token}}/ shape, got {link}"
    );
    assert!(!link.contains("/u/d/"), "the /u/d/ shape is out of scope");
    assert_eq!(created["obj_name"], "/");

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{token}/"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let got: Value = resp.json().await.unwrap();
    assert_keys(
        &got,
        &[
            "token",
            "repo_id",
            "repo_name",
            "path",
            "obj_name",
            "view_cnt",
            "ctime",
            "link",
            "username",
            "expire_date",
            "is_expired",
            "has_password",
            "description",
        ],
    );
    assert_eq!(got["token"], token);

    let resp = f
        .client
        .get("/api/v2.1/upload-links/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let items: Value = resp.json().await.unwrap();
    let item = &items.as_array().unwrap()[0];
    assert_keys(
        item,
        &[
            "token",
            "repo_id",
            "path",
            "has_password",
            "expire_at",
            "view_cnt",
            "description",
        ],
    );
    assert_eq!(item["token"], token);
}

/// The v1 typed upload-link answer and both v1 list/create methods.
#[tokio::test]
async fn test_v1_upload_links_shape() {
    let f = TestFixture::new().await;
    let site = f.server.base_url.trim_end_matches('/');

    let resp = f
        .client
        .post_json(
            "/api2/upload-links",
            Some(&f.api_token),
            &json!({ "repo_id": f.repo_id, "path": "/" }),
        )
        .await;
    let status = resp.status();
    let text = resp.text().await.unwrap();
    assert_eq!(status, 200, "v1 create upload-links body: {text}");
    let created: Value = serde_json::from_str(&text).unwrap();
    assert_keys(
        &created,
        &["token", "link", "repo_id", "path", "created_at"],
    );
    assert!(
        created["link"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{site}/u/"))
    );

    let token = created["token"].as_str().unwrap().to_string();
    let resp = f.client.get("/api2/upload-links", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let items: Value = resp.json().await.unwrap();
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_eq!(items[0]["token"], token);

    let resp = f
        .client
        .delete(&format!("/api2/upload-links/{token}"), Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
}
