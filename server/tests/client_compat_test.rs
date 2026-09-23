//! Regression tests for Seafile-client protocol compatibility fixes.
//!
//! Every test here pins down a deviation from the official clients' or the
//! reference server's contract that was fixed:
//!
//! * `fs-id-list` must never advertise an object `pack-fs` cannot serve (the
//!   empty-directory `EMPTY_SHA1` sentinel), and `pack-fs` must not answer a
//!   short 200 — seaf-daemon re-requests every missing id with no sleep, so a
//!   silent subset became an infinite download loop.
//! * uploads without `replace=1` must keep both files (`name (1).ext`), not
//!   overwrite.
//! * zero-byte uploads must still be committed.
//! * `/upload-api` and `/update-api` answer tab-separated plain-text file ids
//!   unless `ret-json` is present; `-aj` endpoints always answer JSON.
//! * `op=downloadblks`, `op=download|view`, `reloaddir=true`, multipart
//!   `zip-task`, `sub_repo_id` and the 441/442 status codes match upstream.

mod common;

use common::TestFixture;
use infra::serialization::pack_fs;

// ── Helpers ───────────────────────────────────────────────────────────

/// Fetch an `/upload-api/...` URL for the fixture's repo.
async fn upload_link(f: &TestFixture) -> String {
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "upload-link failed");
    resp.json().await.unwrap()
}

/// Fetch an `/update-api/...` URL for the fixture's repo.
async fn update_link(f: &TestFixture) -> String {
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/update-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "update-link failed");
    resp.json().await.unwrap()
}

fn upload_form(name: &str, data: &[u8], parent_dir: &str) -> reqwest::multipart::Form {
    let part = reqwest::multipart::Part::bytes(data.to_vec()).file_name(name.to_string());
    reqwest::multipart::Form::new()
        .part("file", part)
        .text("parent_dir", parent_dir.to_string())
}

fn update_form(name: &str, path: &str, data: &[u8]) -> reqwest::multipart::Form {
    let part = reqwest::multipart::Part::bytes(data.to_vec()).file_name(name.to_string());
    reqwest::multipart::Form::new()
        .part("file", part)
        .text("target_file", path.to_string())
}

/// Names in a directory listing.
async fn dir_names(f: &TestFixture, path: &str) -> Vec<String> {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200, "list_dir failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body.as_array()
        .expect("listing must be an array")
        .iter()
        .filter_map(|e| e["name"].as_str().map(str::to_string))
        .collect()
}

/// The `size` of a named entry in a directory listing.
async fn entry_size(f: &TestFixture, path: &str, name: &str) -> Option<i64> {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200, "list_dir failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body.as_array()?
        .iter()
        .find(|e| e["name"] == name)
        .and_then(|e| e["size"].as_i64())
}

// ── P1.1: fs-id-list / pack-fs ────────────────────────────────────────

/// A new empty directory must not be advertised as a downloadable object: it is
/// the `EMPTY_SHA1` sentinel with no stored fs object, and advertising it made
/// seaf-daemon spin forever re-requesting it.
#[tokio::test]
async fn test_diff_fs_id_list_excludes_empty_dir_sentinel() {
    let f = TestFixture::new().await;

    // Commit A: a file so the root tree is non-trivial.
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "a.txt", b"a")
        .await;
    assert!(up.status().is_success());
    let a = head_commit(&f).await;

    // Commit B: an empty directory (the sentinel dirent).
    let resp = f
        .client
        .create_dir_multipart(&f.api_token, &f.repo_id, "/emptydir")
        .await;
    assert!(resp.status().is_success(), "mkdir failed");
    let b = head_commit(&f).await;
    assert_ne!(a, b, "creating a directory must produce a new commit");

    let resp = f
        .client
        .fs_id_list_with_client(&f.sync_token, &f.repo_id, &b, &a)
        .await;
    assert_eq!(resp.status(), 200);
    let ids: Vec<String> = resp
        .json::<serde_json::Value>()
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();

    assert!(
        !ids.is_empty(),
        "the changed root object itself must be advertised"
    );
    assert!(
        !ids.iter().any(|id| id == &"0".repeat(40)),
        "EMPTY_SHA1 must never appear in fs-id-list, got {ids:?}"
    );

    // Everything advertised must actually come back from pack-fs.
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let resp = f.client.pack_fs(&f.sync_token, &f.repo_id, &refs).await;
    assert_eq!(resp.status(), 200, "pack-fs failed");
    let packed = resp.bytes().await.unwrap();
    let returned = pack_fs::decode_pack_fs_entries(&packed).unwrap();
    let returned_ids: Vec<&str> = returned.iter().map(|(id, _)| id.as_str()).collect();
    for id in &ids {
        assert!(
            returned_ids.contains(&id.as_str()),
            "fs-id-list advertised {id} but pack-fs omitted it"
        );
    }
}

/// A requested object the server cannot serve must fail the request rather than
/// answer a short 200 (which the client treats as "retry immediately").
#[tokio::test]
async fn test_pack_fs_missing_object_is_not_a_short_200() {
    let f = TestFixture::new().await;
    let missing = "0123456789abcdef0123456789abcdef01234567";

    let resp = f
        .client
        .pack_fs(&f.sync_token, &f.repo_id, &[missing])
        .await;
    assert!(
        !resp.status().is_success(),
        "pack-fs must not answer 200 for an id it cannot serve (got {})",
        resp.status()
    );
}

async fn head_commit(f: &TestFixture) -> String {
    let resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    resp.json::<serde_json::Value>().await.unwrap()["head_commit_id"]
        .as_str()
        .unwrap()
        .to_string()
}

// ── P1.2 / P1.3: replace + zero-byte ─────────────────────────────────

/// Uploading a name that already exists without `replace=1` keeps both files,
/// like seafile's `genUniqueName` — it must not overwrite.
#[tokio::test]
async fn test_upload_without_replace_keeps_both() {
    let f = TestFixture::new().await;

    let url = upload_link(&f).await;
    let resp = f
        .client
        .post_multipart_url(&url, upload_form("keep.txt", b"one", "/"))
        .await;
    assert_eq!(resp.status(), 200);

    let url = upload_link(&f).await;
    let resp = f
        .client
        .post_multipart_url(&url, upload_form("keep.txt", b"two", "/"))
        .await;
    assert_eq!(resp.status(), 200);

    let names = dir_names(&f, "/").await;
    assert!(names.contains(&"keep.txt".to_string()), "got {names:?}");
    assert!(
        names.contains(&"keep (1).txt".to_string()),
        "a same-name upload without replace must be renamed, got {names:?}"
    );
}

/// `replace=1` (query or form) overwrites in place.
#[tokio::test]
async fn test_upload_with_replace_overwrites() {
    let f = TestFixture::new().await;

    let url = upload_link(&f).await;
    let resp = f
        .client
        .post_multipart_url(&url, upload_form("rep.txt", b"one", "/"))
        .await;
    assert_eq!(resp.status(), 200);

    // Query form: the upload-link endpoint echoes `?replace=1` onto the URL.
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/&replace=1", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let url: String = resp.json().await.unwrap();
    assert!(url.ends_with("?replace=1"), "got {url}");

    let resp = f
        .client
        .post_multipart_url(&url, upload_form("rep.txt", b"two", "/"))
        .await;
    assert_eq!(resp.status(), 200);

    let names = dir_names(&f, "/").await;
    assert!(names.contains(&"rep.txt".to_string()));
    assert!(
        !names.contains(&"rep (1).txt".to_string()),
        "replace=1 must overwrite, got {names:?}"
    );
}

/// An unusable `replace` value is a 400, matching upstream's `ParseInt` check.
#[tokio::test]
async fn test_upload_invalid_replace_is_rejected() {
    let f = TestFixture::new().await;
    let url = upload_link(&f).await;

    let form = upload_form("bad.txt", b"x", "/").text("replace", "abc");
    let resp = f.client.post_multipart_url(&url, form).await;
    assert_eq!(resp.status(), 400, "invalid replace must be rejected");
}

/// A zero-byte upload must still create the entry (fs id `EMPTY_SHA1`).
#[tokio::test]
async fn test_zero_byte_upload_creates_entry() {
    let f = TestFixture::new().await;
    let url = upload_link(&f).await;

    let resp = f
        .client
        .post_multipart_url(&url, upload_form("empty.bin", b"", "/"))
        .await;
    assert_eq!(resp.status(), 200, "empty upload failed");

    let names = dir_names(&f, "/").await;
    assert!(
        names.contains(&"empty.bin".to_string()),
        "a zero-byte file must still appear, got {names:?}"
    );
    assert_eq!(entry_size(&f, "/", "empty.bin").await, Some(0));
}

// ── P2.1: ret-json framing ───────────────────────────────────────────

/// Without `ret-json` the body is the bare 40-hex file id — the Android and
/// desktop clients store it verbatim.
#[tokio::test]
async fn test_upload_api_returns_plain_id_without_ret_json() {
    let f = TestFixture::new().await;
    let url = upload_link(&f).await;

    let resp = f
        .client
        .post_multipart_url(&url, upload_form("plain.txt", b"x", "/"))
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body.len(), 40, "expected a bare 40-hex id, got {body:?}");
    assert!(body.bytes().all(|b| b.is_ascii_hexdigit()));
}

/// `ret-json` (query or form, any value) switches the body to the JSON array the
/// iOS client parses.
#[tokio::test]
async fn test_upload_api_returns_json_with_ret_json() {
    let f = TestFixture::new().await;

    // Query form, as the Android chunked uploader and iOS build it.
    let url = upload_link(&f).await;
    let sep = if url.contains('?') { '&' } else { '?' };
    let resp = f
        .client
        .post_multipart_url(
            &format!("{url}{sep}ret-json=true"),
            upload_form("rj.txt", b"x", "/"),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array(), "expected a JSON array, got {body:?}");
    assert_eq!(
        body[0]["id"].as_str().map(str::len),
        Some(40),
        "expected an id in the first element, got {body:?}"
    );

    // Form-field form.
    let url = upload_link(&f).await;
    let resp = f
        .client
        .post_multipart_url(
            &url,
            upload_form("rj2.txt", b"x", "/").text("ret-json", "1"),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array(), "expected a JSON array, got {body:?}");
}

// ── P2.7: update-api id framing and 441 ──────────────────────────────

/// `/update-api` also answers a bare id without `ret-json`.
#[tokio::test]
async fn test_update_api_returns_plain_id() {
    let f = TestFixture::new().await;
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "u.txt", b"old")
        .await;
    assert!(up.status().is_success());

    let url = update_link(&f).await;
    let resp = f
        .client
        .post_multipart_url(&url, update_form("u.txt", "/u.txt", b"new"))
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body.len(), 40, "expected a bare id, got {body:?}");
}

/// Updating a target that does not exist is 441, not a silent create.
#[tokio::test]
async fn test_update_api_missing_target_is_441() {
    let f = TestFixture::new().await;
    let url = update_link(&f).await;

    let resp = f
        .client
        .post_multipart_url(&url, update_form("nope.txt", "/nope.txt", b"x"))
        .await;
    assert_eq!(resp.status(), 441, "missing target must be 441");
}

// ── P2.2: op=downloadblks ────────────────────────────────────────────

#[tokio::test]
async fn test_file_op_downloadblks_shape() {
    let f = TestFixture::new().await;
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "blks.txt", b"hello blocks")
        .await;
    assert!(up.status().is_success());

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
    let oid = resp
        .headers()
        .get("oid")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body: serde_json::Value = resp.json().await.unwrap();

    let file_id = body["file_id"].as_str().expect("file_id missing");
    assert_eq!(oid.as_deref(), Some(file_id), "oid header must match");
    let blklist = body["blklist"].as_array().expect("blklist missing");
    assert!(!blklist.is_empty(), "expected at least one block");
    assert_eq!(body["encrypted"], serde_json::json!(false));
    assert_eq!(body["enc_version"], serde_json::json!(0));
}

// ── P2.3: reloaddir ──────────────────────────────────────────────────

/// `reloaddir=true` on mkdir returns the parent listing as a bare array, which
/// is the only shape the iOS client's `SeafDir.handleData:` accepts.
#[tokio::test]
async fn test_mkdir_reloaddir_returns_bare_listing() {
    let f = TestFixture::new().await;

    let form = reqwest::multipart::Form::new().text("operation", "mkdir");
    let resp = f
        .client
        .post_multipart(
            &format!("/api2/repos/{}/dir/?p=/rdir&reloaddir=true", f.repo_id),
            Some(&f.api_token),
            form,
        )
        .await;
    assert_eq!(resp.status(), 200, "mkdir reloaddir failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body
        .as_array()
        .unwrap_or_else(|| panic!("expected a bare array, got {body:?}"));
    assert!(
        arr.iter().any(|e| e["name"] == "rdir"),
        "the new folder must be listed, got {body:?}"
    );
}

/// `reloaddir=true` on a file rename returns the parent listing containing the
/// new name — without it iOS reported a successful rename as a failure.
#[tokio::test]
async fn test_file_rename_reloaddir_returns_bare_listing() {
    let f = TestFixture::new().await;
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "old.txt", b"x")
        .await;
    assert!(up.status().is_success());

    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/file/?p=/old.txt&reloaddir=true", f.repo_id),
            Some(&f.api_token),
            &[("operation", "rename"), ("newname", "new.txt")],
        )
        .await;
    assert_eq!(resp.status(), 200, "rename reloaddir failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body
        .as_array()
        .unwrap_or_else(|| panic!("expected a bare array, got {body:?}"));
    assert!(
        arr.iter().any(|e| e["name"] == "new.txt"),
        "renamed entry must be listed, got {body:?}"
    );
    assert!(
        !arr.iter().any(|e| e["name"] == "old.txt"),
        "old name must be gone, got {body:?}"
    );
}

// ── P2.4: op=download|view ───────────────────────────────────────────

#[tokio::test]
async fn test_repos_files_op_download_and_view() {
    let f = TestFixture::new().await;
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "cd.txt", b"data")
        .await;
    assert!(up.status().is_success());

    let resp = f
        .client
        .get(
            &format!("/repos/{}/files/cd.txt?op=download", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let disposition = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        disposition.starts_with("attachment"),
        "op=download must force attachment, got {disposition:?}"
    );

    let resp = f
        .client
        .get(
            &format!("/repos/{}/files/cd.txt?op=view", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let disposition = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        disposition.starts_with("inline"),
        "op=view must be inline, got {disposition:?}"
    );
}

/// An unsatisfiable `Range` is 416, not a silent full-body 200.
#[tokio::test]
async fn test_unsatisfiable_range_is_416() {
    let f = TestFixture::new().await;
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "rng.txt", b"0123456789")
        .await;
    assert!(up.status().is_success());

    let resp = f
        .client
        .get_with_range(
            &format!("/repos/{}/files/rng.txt", f.repo_id),
            Some(&f.api_token),
            "bytes=1000-2000",
        )
        .await;
    assert_eq!(resp.status(), 416, "headers: {:?}", resp.headers());
    let content_range = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        content_range.as_deref(),
        Some("bytes */10"),
        "headers: {:?}",
        resp.headers()
    );
}

// ── P2.5: zip-task multipart ─────────────────────────────────────────

#[tokio::test]
async fn test_zip_task_accepts_multipart_form_data() {
    let f = TestFixture::new().await;
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "z.txt", b"zip me")
        .await;
    assert!(up.status().is_success());

    let form = reqwest::multipart::Form::new()
        .text("parent_dir", "/")
        .text("dirents", "z.txt");
    let resp = f
        .client
        .post_multipart(
            &format!("/api/v2.1/repos/{}/zip-task/", f.repo_id),
            Some(&f.api_token),
            form,
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "multipart zip-task failed: {:?}",
        resp.text().await
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["zip_token"].as_str().is_some_and(|t| !t.is_empty()),
        "expected a zip_token, got {body:?}"
    );
}

// ── P2.6: sub_repo_id ────────────────────────────────────────────────

#[tokio::test]
async fn test_sub_repo_response_includes_sub_repo_id() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .create_dir_multipart(&f.api_token, &f.repo_id, "/subsrc")
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/dir/sub_repo/?p=/subsrc", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "sub_repo failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let id = body["sub_repo_id"]
        .as_str()
        .unwrap_or_else(|| panic!("sub_repo_id missing, got {body:?}"));
    assert_eq!(id, body["id"].as_str().unwrap());
}

// ── P3: upload/update link host ──────────────────────────────────────

/// With `site_url` left at its built-in loopback default, the upload link must
/// echo the address the client actually used — the same rule download links
/// follow. Otherwise a LAN client is told to upload to its own loopback.
#[tokio::test]
async fn test_upload_link_echoes_request_host_when_site_url_is_default() {
    // `new_with_encrypted_library_config` is the generic ServerConfig-tweak
    // fixture; only `site_url` is touched here.
    let f = TestFixture::new_with_encrypted_library_config(|cfg| {
        cfg.site_url = "http://127.0.0.1:8082".to_string();
    })
    .await;

    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!(
            "{}/api2/repos/{}/upload-link/?p=/",
            f.server.base_url, f.repo_id
        ))
        .header("Host", "192.168.1.9:8082")
        .bearer_auth(&f.api_token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let url: String = resp.json().await.unwrap();
    assert!(
        url.starts_with("http://192.168.1.9:8082/upload-api/"),
        "upload link must echo the request host, got {url}"
    );
}
