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
        url.contains("/files/"),
        "download URL must target /files, got {url}"
    );

    let resp = f.client.get(path_of(&url, "/files/"), None).await;
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
    // Seafile's block URL is `/blks/{token}/{block_id}`: two segments, with the
    // file id carried by the token rather than spelled in the path.
    let rest = url
        .split("/blks/")
        .nth(1)
        .expect("the URL names the /blks/ surface");
    assert_eq!(
        rest.trim_end_matches('/').split('/').count(),
        2,
        "the block URL must keep the fileserver's two-segment shape, got {url}"
    );

    let resp = f.client.get(path_of(&url, "/blks/"), None).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("max-age=3600"),
        "blocks are immutable and briefly cacheable"
    );
    assert_eq!(resp.bytes().await.unwrap().as_ref(), data);

    // A block token must not unlock the whole-file endpoint.
    let block_token = url
        .split("/blks/")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .expect("block token");
    let resp = f
        .client
        .get(&format!("/files/{block_token}/file.bin"), None)
        .await;
    assert_eq!(resp.status(), 400);
    assert!(
        resp.text()
            .await
            .unwrap()
            .contains("not valid for download")
    );
}

/// The self-contained block URL this server minted before adopting Seafile's
/// shape still serves, from its `/api/transfers/blks/` home: the file id travels
/// in the path there, and links handed out under it stay valid for their hour.
#[tokio::test]
async fn test_block_download_self_contained_legacy_path() {
    let f = TestFixture::new().await;
    let data = b"legacy self-contained block";
    upload(&f, "/", "legacy-block.bin", data).await;
    let file_id = file_detail_id(&f, "/legacy-block.bin").await;
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
    let token = url
        .split("/blks/")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .expect("block token");

    let resp = f
        .client
        .get(
            &format!("/api/transfers/blks/{token}/{file_id}/{block_id}"),
            None,
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "the pre-move block URL must keep serving its block"
    );
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
    let path = path_of(&url, "/files/").to_string();

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
                "/api/keys/",
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

    let resp = http
        .get(&url)
        .basic_auth(&f.email, Some(&key))
        .send()
        .await
        .unwrap();
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("WebDAV GET must carry an ETag")
        .to_string();
    let resp = http
        .get(&url)
        .basic_auth(&f.email, Some(&key))
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 304);
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
                "/api2/repos/{}/file/revision/?p=/hist.txt&commit_id={commit}",
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

fn raw() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// A stable content URL revalidates: `If-None-Match` is evaluated before `Range`.
#[tokio::test]
async fn test_stable_url_revalidates_before_range() {
    let f = TestFixture::new().await;
    let data: Vec<u8> = (0..32u8).collect();
    upload(&f, "/", "cond.bin", &data).await;

    let http = raw();
    let url = format!("{}/repos/{}/files/cond.bin", f.server.base_url, f.repo_id);
    let resp = http
        .get(&url)
        .bearer_auth(&f.api_token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("private, no-cache"),
        "a stable URL must revalidate"
    );
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("ETag missing")
        .to_string();

    let resp = http
        .get(&url)
        .bearer_auth(&f.api_token)
        .header("if-none-match", &etag)
        .header("range", "bytes=0-4")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        304,
        "If-None-Match must be evaluated before Range"
    );
    assert_eq!(
        resp.headers().get("etag").and_then(|v| v.to_str().ok()),
        Some(etag.as_str())
    );
    assert!(resp.bytes().await.unwrap().is_empty());
}

/// The token URL carries a short cache window and answers conditional requests.
#[tokio::test]
async fn test_download_api_revalidates() {
    let f = TestFixture::new().await;
    upload(&f, "/", "cond2.bin", &(0..64u8).collect::<Vec<u8>>()).await;

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/file/?p=/cond2.bin&op=download", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let url: String = resp.json().await.unwrap();
    let etag = {
        let resp = raw().get(&url).send().await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers()
                .get("cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("max-age=3600")
        );
        resp.headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .expect("ETag missing")
            .to_string()
    };

    let resp = raw()
        .get(&url)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 304);
    assert!(resp.bytes().await.unwrap().is_empty());
}

/// Share downloads are stable URLs too: they expose a validator and revalidate.
#[tokio::test]
async fn test_share_download_revalidates() {
    let f = TestFixture::new().await;
    upload(&f, "/", "shared-c.bin", b"share validator").await;
    let token = share_link(&f, "/shared-c.bin").await;

    let url = format!("{}/f/{token}/?dl=1", f.server.base_url);
    let resp = raw().get(&url).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("public, no-cache")
    );
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("share downloads must carry a validator")
        .to_string();

    let resp = raw()
        .get(&url)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 304);
}
