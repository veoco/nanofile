//! The download contract matrix: the shapes and headers every byte-serving
//! endpoint promises, pinned before the response pipeline is converged.
//!
//! Cache policy and conditional-request ordering are deliberately left to the
//! phase that changes them; this file pins the stable contract (status codes,
//! bodies, URL shapes, Range, disposition).

use crate::common;

use common::TestFixture;
use reqwest::Method;

/// Cut the path out of an absolute capability URL so `TestClient` can request it.
fn path_of<'a>(url: &'a str, marker: &str) -> &'a str {
    url.find(marker).map(|i| &url[i..]).unwrap_or(url)
}

fn sha1_hex(data: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

async fn upload(f: &TestFixture, dir: &str, name: &str, data: &[u8]) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, dir, name, data)
        .await;
    assert!(
        resp.status().is_success(),
        "upload {dir}/{name} failed: {}",
        resp.status()
    );
}

async fn mkdir(f: &TestFixture, path: &str) {
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, path).await;
    assert!(resp.status().is_success(), "mkdir {path} failed");
}

async fn file_detail_id(f: &TestFixture, path: &str) -> String {
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/file/detail/?p={}", f.repo_id, path),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "file detail failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["id"].as_str().unwrap().to_string()
}

async fn share_link(f: &TestFixture, path: &str) -> String {
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": path }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["token"].as_str().unwrap().to_string()
}

/// v1 `?op=download` answers a JSON string URL plus the `oid` header; fetching
/// that URL streams the bytes.
#[tokio::test]
async fn test_v1_download_returns_url_and_oid() {
    let f = TestFixture::new().await;
    upload(&f, "/", "dl.txt", b"download me").await;

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/file/?p=/dl.txt&op=download", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "op=download failed");
    assert!(
        resp.headers().contains_key("oid"),
        "the download answer must carry an oid header"
    );
    let url: String = resp.json().await.unwrap();
    assert!(
        url.contains("/download-api/"),
        "download URL must target /download-api, got {url}"
    );

    let resp = f.client.get(path_of(&url, "/download-api/"), None).await;
    assert_eq!(resp.status(), 200);
    let disposition = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        disposition.starts_with("attachment"),
        "the token URL must download, not inline, got {disposition:?}"
    );
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"download me");
}

/// v1 `?op=downloadblks` keeps seahub's object shape, including the zero-byte case.
#[tokio::test]
async fn test_v1_downloadblks_shape() {
    let f = TestFixture::new().await;
    let data = b"block list payload";
    upload(&f, "/", "blks.txt", data).await;

    let resp = f
        .client
        .get(
            &format!(
                "/api2/repos/{}/file/?p=/blks.txt&op=downloadblks",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "op=downloadblks failed");
    assert!(resp.headers().contains_key("oid"), "oid header missing");
    let body: serde_json::Value = resp.json().await.unwrap();
    let expected_block = sha1_hex(data);
    assert_eq!(body["file_id"].as_str().map(str::len), Some(40));
    assert_eq!(body["blklist"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["blklist"][0].as_str(), Some(expected_block.as_str()));
    assert_eq!(body["encrypted"], serde_json::json!(false));
    assert_eq!(body["enc_version"], serde_json::json!(0));

    upload(&f, "/", "empty.bin", b"").await;
    let resp = f
        .client
        .get(
            &format!(
                "/api2/repos/{}/file/?p=/empty.bin&op=downloadblks",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["blklist"].as_array().map(Vec::len), Some(0));
}

/// The block download link answers a URL; that URL serves exactly the block bytes.
#[tokio::test]
async fn test_block_download_link_serves_block() {
    let f = TestFixture::new().await;
    let data = b"hello block download";
    upload(&f, "/", "block.bin", data).await;
    let file_id = file_detail_id(&f, "/block.bin").await;
    let block_id = sha1_hex(data);

    let resp = f
        .client
        .get(
            &format!(
                "/api2/repos/{}/files/{}/blks/{}/download-link/",
                f.repo_id, file_id, block_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let url: String = resp.json().await.unwrap();
    assert!(
        url.contains(&block_id),
        "URL must name the block, got {url}"
    );

    let resp = f.client.get(path_of(&url, "/blks/"), None).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), data);
}

/// The token URL supports resumable ranges and reports an unsatisfiable range.
#[tokio::test]
async fn test_download_api_range_and_416() {
    let f = TestFixture::new().await;
    let data: Vec<u8> = (0..64u8).collect();
    upload(&f, "/", "range.bin", &data).await;

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/file/?p=/range.bin&op=download", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let url: String = resp.json().await.unwrap();
    let path = path_of(&url, "/download-api/").to_string();

    let resp = f.client.get_with_range(&path, None, "bytes=10-19").await;
    assert_eq!(resp.status(), 206);
    assert_eq!(
        resp.headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok()),
        Some("bytes 10-19/64")
    );
    assert_eq!(resp.bytes().await.unwrap().as_ref(), &data[10..20]);

    let resp = f.client.get_with_range(&path, None, "bytes=999-1000").await;
    assert_eq!(resp.status(), 416);
    assert_eq!(
        resp.headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok()),
        Some("bytes */64")
    );
}

/// The web content endpoint honours `op`/`dl` for the disposition and serves ranges.
#[tokio::test]
async fn test_repos_files_ops_and_range() {
    let f = TestFixture::new().await;
    let data: Vec<u8> = (0..40u8).collect();
    upload(&f, "/", "ops.bin", &data).await;

    for (query, expected) in [
        ("?op=download", "attachment"),
        ("?op=view", "inline"),
        ("?dl=1", "attachment"),
        ("", "inline"),
    ] {
        // The no-query case has no disposition header; `unwrap_or("inline")` keeps
        // the loop honest for the three explicit ones.
        let resp = f
            .client
            .get(
                &format!("/repos/{}/files/ops.bin{query}", f.repo_id),
                Some(&f.api_token),
            )
            .await;
        assert_eq!(resp.status(), 200, "query {query:?}");
        let disposition = resp
            .headers()
            .get(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("inline")
            .to_string();
        assert!(
            disposition.starts_with(expected),
            "query {query:?}: expected {expected}, got {disposition:?}"
        );
    }

    let resp = f
        .client
        .get_with_range(
            &format!("/repos/{}/files/ops.bin", f.repo_id),
            Some(&f.api_token),
            "bytes=5-9",
        )
        .await;
    assert_eq!(resp.status(), 206);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), &data[5..10]);
}

/// A shared file downloads with Range support through `?dl=1`.
#[tokio::test]
async fn test_share_file_download_range() {
    let f = TestFixture::new().await;
    let data: Vec<u8> = (0..50u8).collect();
    upload(&f, "/", "shared.bin", &data).await;
    let token = share_link(&f, "/shared.bin").await;

    let resp = f.client.get(&format!("/f/{token}/?dl=1"), None).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), &data[..]);

    let resp = f
        .client
        .get_with_range(&format!("/f/{token}/?dl=1"), None, "bytes=20-29")
        .await;
    assert_eq!(resp.status(), 206);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), &data[20..30]);
}

/// A file inside a shared folder downloads through its path.
#[tokio::test]
async fn test_share_dir_file_download() {
    let f = TestFixture::new().await;
    mkdir(&f, "/sub").await;
    upload(&f, "/sub", "inner.txt", b"inner content").await;
    let token = share_link(&f, "/sub").await;

    let resp = f
        .client
        .get(&format!("/d/{token}/files/inner.txt"), None)
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"inner content");
}

/// WebDAV GET streams the bytes and HEAD returns the same headers without a body.
#[tokio::test]
async fn test_webdav_get_and_head() {
    let f = TestFixture::new().await;
    upload(&f, "/", "dav.txt", b"dav content").await;

    let key: String = {
        let resp = f
            .client
            .post_json(
                "/api2/api-keys/",
                Some(&f.api_token),
                &serde_json::json!({
                    "name": "dav-read",
                    "capabilities": ["webdav.read"],
                    "repo_permissions": [{ "repo_id": f.repo_id, "permission": "r" }],
                    "never": true,
                }),
            )
            .await;
        assert_eq!(resp.status(), 200, "mint webdav key failed");
        let body: serde_json::Value = resp.json().await.unwrap();
        body["key"].as_str().unwrap().to_string()
    };

    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("{}/dav/{}/dav.txt", f.server.base_url, f.repo_id);
    let resp = http
        .get(&url)
        .basic_auth(&f.email, Some(&key))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"dav content");

    let m = Method::from_bytes(b"HEAD").unwrap();
    let resp = http
        .request(m, &url)
        .basic_auth(&f.email, Some(&key))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()),
        Some("11")
    );
    assert!(
        resp.bytes().await.unwrap().is_empty(),
        "HEAD must not send a body"
    );
}

/// History revisions stream the old bytes under `/api/v2.1/.../file/revision/`.
#[tokio::test]
async fn test_revision_download() {
    let f = TestFixture::new().await;
    upload(&f, "/", "hist.txt", b"first-content").await;
    upload(&f, "/", "hist.txt", b"second-content").await;

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/file/history/?p=/hist.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let commit = body["data"][1]["commit_id"].as_str().unwrap().to_string();

    let resp = f
        .client
        .get(
            &format!(
                "/api/v2.1/repos/{}/file/revision/?p=/hist.txt&commit_id={commit}",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"first-content");
}

/// A zip token is minted once, the archive streams, and the token is single-use.
#[tokio::test]
async fn test_zip_task_roundtrip() {
    let f = TestFixture::new().await;
    upload(&f, "/", "z.txt", b"zip me").await;

    let resp = f
        .client
        .zip_task(&f.api_token, &f.repo_id, "/", &["z.txt"])
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["zip_token"].as_str().unwrap().to_string();
    assert!(!token.is_empty());

    let resp = f.client.zip_download(&token).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
        "application/zip"
    );
    let bytes = resp.bytes().await.unwrap();
    assert!(bytes.starts_with(b"PK\x03\x04"), "not a zip archive");

    let resp = f.client.zip_download(&token).await;
    assert_eq!(resp.status(), 404, "a zip token must be single-use");
}

/// The desktop client reads these fields to clone a library; they must exist.
#[tokio::test]
async fn test_download_info_fields() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/download-info/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    for field in [
        "repo_id",
        "repo_name",
        "token",
        "email",
        "encrypted",
        "enc_version",
        "repo_version",
        "permission",
    ] {
        assert!(
            body.get(field).is_some(),
            "download-info is missing {field}: {body}"
        );
    }
    assert_eq!(body["repo_id"].as_str(), Some(f.repo_id.as_str()));
    assert!(!body["token"].as_str().unwrap_or_default().is_empty());
}
