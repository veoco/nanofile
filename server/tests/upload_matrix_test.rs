//! Upload/update contract matrix.
//!
//! One test per endpoint family, asserting the **decided** contract: upstream's
//! wire shape where it matches, the documented Nanofile extension otherwise
//! (`docs/reference/api/transfers.md`). Nothing here asserts "whatever the code
//! happens to do today" without a decision behind it.
//!
//! Every endpoint here authenticates with a minted upload/update token (the
//! browser's `from=web` link points at the same token family), so no cookie-jar
//! client is involved.

use crate::common::TestFixture;
use reqwest::multipart::{Form, Part};

/// The empty-file sentinel every upload path answers for a zero-byte commit.
const EMPTY_SHA1: &str = infra::common::EMPTY_SHA1;

fn file_part(name: &str, bytes: &[u8]) -> Part {
    Part::bytes(bytes.to_vec()).file_name(name.to_string())
}

/// Build a multipart body: text fields in order, then one `file` part per entry.
fn form(fields: &[(&str, &str)], files: &[(&str, &[u8])]) -> Form {
    let mut form = Form::new();
    for (name, value) in fields {
        form = form.text((*name).to_string(), (*value).to_string());
    }
    for (name, bytes) in files {
        form = form.part("file", file_part(name, bytes));
    }
    form
}

fn is_file_id(body: &str) -> bool {
    body.len() == 40 && body.chars().all(|c| c.is_ascii_hexdigit())
}

/// Mint an upload token for `p`; the returned URL is absolute (`from=web` selects
/// the browser form `/upload-aj/{token}`, otherwise `/upload-api/{token}`).
async fn upload_link(f: &TestFixture, p: &str, web: bool) -> String {
    let from = if web { "&from=web" } else { "" };
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p={}{}", f.repo_id, p, from),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "upload-link request failed");
    resp.json::<String>().await.unwrap()
}

/// Mint an update token for `p`; the desktop URL `/update-api/{token}`.
async fn update_link(f: &TestFixture, p: &str) -> String {
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/update-link/?p={}", f.repo_id, p),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "update-link request failed");
    resp.json::<String>().await.unwrap()
}

/// Mint the browser form of an update token (`/update-aj/{token}`), which the
/// web UI uses; `from=web` selects that view, as it does for uploads.
async fn update_link_web(f: &TestFixture, p: &str) -> String {
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/update-link/?p={}&from=web", f.repo_id, p),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "update-link (web) request failed");
    resp.json::<String>().await.unwrap()
}

async fn dir_names(f: &TestFixture, path: &str) -> Vec<String> {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200, "listing {path} failed");
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    entries
        .iter()
        .map(|e| e["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Whether `path` exists, by listing its parent. Used before downloading: the
/// fixture's `download_file` asserts the download link exists, so it panics on a
/// missing file instead of reporting absence.
async fn exists(f: &TestFixture, path: &str) -> bool {
    let (parent, name) = path
        .rsplit_once('/')
        .map(|(p, n)| (if p.is_empty() { "/" } else { p }, n))
        .unwrap_or(("/", path));
    dir_names(f, parent).await.iter().any(|n| n == name)
}

async fn download(f: &TestFixture, path: &str) -> Option<Vec<u8>> {
    if !exists(f, path).await {
        return None;
    }
    let resp = f.client.download_file(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200, "downloading {path} failed");
    Some(resp.bytes().await.unwrap().to_vec())
}

/// `-api` answers the id as plain text; `ret-json` switches to the JSON array.
/// The desktop and Android clients store the plain body verbatim.
#[tokio::test]
async fn upload_api_answers_plain_id_then_ret_json() {
    let f = TestFixture::new().await;
    let url = upload_link(&f, "/", false).await;

    let resp = f
        .client
        .post_multipart_url(
            &url,
            form(&[("parent_dir", "/")], &[("plain.txt", b"hello")]),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        is_file_id(&body),
        "plain body must be the file id, got {body:?}"
    );
    assert_eq!(
        download(&f, "/plain.txt").await.as_deref(),
        Some(&b"hello"[..])
    );

    let resp = f
        .client
        .post_multipart_url(
            &format!("{url}?ret-json=1"),
            form(&[("parent_dir", "/")], &[("json.txt", b"world")]),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array(), "ret-json must answer an array, got {body}");
    assert_eq!(body[0]["name"], "json.txt");
    assert_eq!(body[0]["size"], 5);
}

/// `-aj` (the browser form target) always answers the JSON array, even without
/// `ret-json`.
#[tokio::test]
async fn upload_aj_answers_array() {
    let f = TestFixture::new().await;
    let url = upload_link(&f, "/", true).await;

    let resp = f
        .client
        .post_multipart_url(&url, form(&[("parent_dir", "/")], &[("aj.txt", b"array")]))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body.is_array(),
        "the -aj form must answer an array, got {body}"
    );
    assert_eq!(body[0]["name"], "aj.txt");
    assert_eq!(
        download(&f, "/aj.txt").await.as_deref(),
        Some(&b"array"[..])
    );
}

/// A multi-file `-api` request answers tab-separated ids; every part is committed.
#[tokio::test]
async fn upload_api_joins_multi_file_ids_with_tabs() {
    let f = TestFixture::new().await;
    let url = upload_link(&f, "/", false).await;

    let resp = f
        .client
        .post_multipart_url(
            &url,
            form(
                &[("parent_dir", "/")],
                &[("one.txt", b"1"), ("two.txt", b"2")],
            ),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    let ids: Vec<&str> = body.split('\t').collect();
    assert_eq!(ids.len(), 2, "two parts must answer two ids, got {body:?}");
    assert!(ids.iter().all(|id| is_file_id(id)));
    assert_eq!(download(&f, "/one.txt").await.as_deref(), Some(&b"1"[..]));
    assert_eq!(download(&f, "/two.txt").await.as_deref(), Some(&b"2"[..]));
}

/// Without `replace`, a name that already exists keeps both entries (the new one
/// gets a unique name); the original content is untouched.
#[tokio::test]
async fn upload_duplicate_name_keeps_both_without_replace() {
    let f = TestFixture::new().await;
    let seed = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "dup.txt", b"original")
        .await;
    assert_eq!(seed.status(), 200, "seeding failed");

    let url = upload_link(&f, "/", false).await;
    let resp = f
        .client
        .post_multipart_url(
            &format!("{url}?ret-json=1"),
            form(&[("parent_dir", "/")], &[("dup.txt", b"second")]),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let committed = body[0]["name"].as_str().unwrap();
    assert_ne!(
        committed, "dup.txt",
        "a kept-both upload must pick a new name"
    );
    assert_eq!(dir_names(&f, "/").await.len(), 2);
    assert_eq!(
        download(&f, "/dup.txt").await.as_deref(),
        Some(&b"original"[..]),
        "the original must not be overwritten"
    );
}

/// `replace` spells `1`/`true` (overwrite) and `0`/`false` (keep both); anything
/// else is a 400, matching the Go fileserver's `strconv.ParseInt` strictness.
#[tokio::test]
async fn upload_replace_spellings() {
    let f = TestFixture::new().await;

    for (name, replace, overwrites) in [
        ("r1.txt", "1", true),
        ("rt.txt", "true", true),
        ("r0.txt", "0", false),
        ("rf.txt", "false", false),
    ] {
        let seed = f
            .client
            .upload_file(&f.api_token, &f.repo_id, "/", name, b"old")
            .await;
        assert_eq!(seed.status(), 200, "seeding {name} failed");

        let url = upload_link(&f, "/", false).await;
        let resp = f
            .client
            .post_multipart_url(
                &url,
                form(
                    &[("parent_dir", "/"), ("replace", replace)],
                    &[(name, b"new")],
                ),
            )
            .await;
        assert_eq!(resp.status(), 200, "replace={replace} must be accepted");
        let stored = download(&f, &format!("/{name}")).await.unwrap();
        if overwrites {
            assert_eq!(stored, b"new", "replace={replace} must overwrite {name}");
        } else {
            assert_eq!(stored, b"old", "replace={replace} must keep {name}");
        }
    }

    let url = upload_link(&f, "/", false).await;
    let resp = f
        .client
        .post_multipart_url(
            &url,
            form(
                &[("parent_dir", "/"), ("replace", "yes")],
                &[("bad.txt", b"x")],
            ),
        )
        .await;
    assert_eq!(resp.status(), 400, "an unparsable replace must be rejected");
}

/// A zero-byte upload commits the `EMPTY_SHA1` sentinel but still creates the
/// entry, as the Go fileserver does.
#[tokio::test]
async fn upload_zero_byte_creates_entry() {
    let f = TestFixture::new().await;
    let url = upload_link(&f, "/", false).await;

    let resp = f
        .client
        .post_multipart_url(&url, form(&[("parent_dir", "/")], &[("empty.txt", b"")]))
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body, EMPTY_SHA1, "zero-byte uploads answer the sentinel id");
    assert!(
        dir_names(&f, "/").await.iter().any(|n| n == "empty.txt"),
        "the empty file must still be listed"
    );
}

/// Every upload entry point rejects a body without a `file` part.
#[tokio::test]
async fn upload_requires_a_file_part() {
    let f = TestFixture::new().await;
    let api = upload_link(&f, "/", false).await;
    let aj = upload_link(&f, "/", true).await;

    for (label, url) in [("api", api), ("aj-token", aj)] {
        let resp = f
            .client
            .post_multipart_url(&url, form(&[("parent_dir", "/")], &[]))
            .await;
        assert_eq!(
            resp.status(),
            400,
            "{label} must reject a body without a file"
        );
    }
}

/// `parent_dir` may not contain `//`, and `relative_path` may not start with `/`.
#[tokio::test]
async fn upload_rejects_malformed_paths() {
    let f = TestFixture::new().await;
    let url = upload_link(&f, "/", false).await;

    for (parent, relative) in [("//", ""), ("/", "/sub")] {
        let resp = f
            .client
            .post_multipart_url(
                &url,
                form(
                    &[("parent_dir", parent), ("relative_path", relative)],
                    &[("bad.txt", b"x")],
                ),
            )
            .await;
        assert_eq!(
            resp.status(),
            400,
            "parent_dir={parent:?} relative_path={relative:?} must be rejected"
        );
    }
}

/// An upload-link token is scoped to the link's directory: the client's
/// `parent_dir` is ignored, not honoured.
#[tokio::test]
async fn upload_link_ignores_client_parent_dir() {
    let f = TestFixture::new().await;
    let dir = f.client.create_dir(&f.api_token, &f.repo_id, "/sub").await;
    assert!(dir.status().is_success(), "creating /sub failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({"repo_id": f.repo_id, "path": "/sub"}),
        )
        .await;
    assert_eq!(resp.status(), 200, "creating the upload link failed");
    let token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{}/upload/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let link = resp.json::<serde_json::Value>().await.unwrap()["upload_link"]
        .as_str()
        .unwrap()
        .to_string();
    let url = format!("{}{}", f.server.base_url, link);

    let resp = f
        .client
        .post_multipart_url(&url, form(&[("parent_dir", "/")], &[("scoped.txt", b"x")]))
        .await;
    assert_eq!(resp.status(), 200);
    assert!(
        download(&f, "/sub/scoped.txt").await.is_some(),
        "the file must land in the link's directory"
    );
    assert!(
        download(&f, "/scoped.txt").await.is_none(),
        "the client parent_dir must not widen the link's scope"
    );
}

/// Upload links are add-only: `replace=1` cannot overwrite what the creator
/// already had in the shared directory.
#[tokio::test]
async fn upload_link_is_add_only() {
    let f = TestFixture::new().await;
    let dir = f.client.create_dir(&f.api_token, &f.repo_id, "/sub").await;
    assert!(dir.status().is_success(), "creating /sub failed");
    let seed = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/sub", "keep.txt", b"creator")
        .await;
    assert_eq!(seed.status(), 200, "seeding failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({"repo_id": f.repo_id, "path": "/sub"}),
        )
        .await;
    let token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{}/upload/", token),
            Some(&f.api_token),
        )
        .await;
    let link = resp.json::<serde_json::Value>().await.unwrap()["upload_link"]
        .as_str()
        .unwrap()
        .to_string();
    let url = format!("{}{}", f.server.base_url, link);

    let resp = f
        .client
        .post_multipart_url(
            &url,
            form(
                &[("parent_dir", "/sub"), ("replace", "1")],
                &[("keep.txt", b"anonymous")],
            ),
        )
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        download(&f, "/sub/keep.txt").await.as_deref(),
        Some(&b"creator"[..]),
        "a link must never overwrite the creator's file"
    );
    assert_eq!(dir_names(&f, "/sub").await.len(), 2);
}

/// Every update endpoint answers 441 when the target does not exist.
#[tokio::test]
async fn update_requires_existing_target() {
    let f = TestFixture::new().await;
    let api = update_link(&f, "/").await;
    let aj = update_link_web(&f, "/").await;

    for (label, url) in [("api", api), ("aj-token", aj)] {
        let resp = f
            .client
            .post_multipart_url(
                &url,
                form(&[("target_file", "/missing.txt")], &[("missing.txt", b"x")]),
            )
            .await;
        assert_eq!(
            resp.status(),
            441,
            "{label} must answer 441 for a missing target"
        );
    }
}

/// An update carries exactly one file part; a second one is a 400 on every
/// update endpoint.
#[tokio::test]
async fn update_rejects_a_second_file() {
    let f = TestFixture::new().await;
    let seed = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "one.txt", b"old")
        .await;
    assert_eq!(seed.status(), 200);
    let api = update_link(&f, "/").await;

    let resp = f
        .client
        .post_multipart_url(
            &api,
            form(
                &[("target_file", "/one.txt")],
                &[("one.txt", b"a"), ("two.txt", b"b")],
            ),
        )
        .await;
    assert_eq!(resp.status(), 400, "-api must reject a second file");
}

/// The update endpoints answer the shapes the desktop client and the web UI read:
/// `-api` the plain id and `-aj` the JSON array.
#[tokio::test]
async fn update_reply_shapes() {
    let f = TestFixture::new().await;
    for name in ["s.txt", "p.txt", "a.txt"] {
        let seed = f
            .client
            .upload_file(&f.api_token, &f.repo_id, "/", name, b"old")
            .await;
        assert_eq!(seed.status(), 200);
    }

    let api = update_link(&f, "/").await;
    let resp = f
        .client
        .post_multipart_url(
            &api,
            form(&[("target_file", "/p.txt")], &[("p.txt", b"new")]),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(is_file_id(&body), "-api answers the plain id, got {body:?}");

    let aj = update_link_web(&f, "/").await;
    let resp = f
        .client
        .post_multipart_url(
            &aj,
            form(&[("target_file", "/a.txt")], &[("a.txt", b"new")]),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array(), "-aj answers an array");
}

/// A zero-byte update truncates the existing file (it does not delete it).
#[tokio::test]
async fn update_zero_byte_truncates() {
    let f = TestFixture::new().await;
    let seed = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "trunc.txt", b"content")
        .await;
    assert_eq!(seed.status(), 200);

    let api = update_link(&f, "/").await;
    let resp = f
        .client
        .post_multipart_url(
            &api,
            form(&[("target_file", "/trunc.txt")], &[("trunc.txt", b"")]),
        )
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(download(&f, "/trunc.txt").await.as_deref(), Some(&b""[..]));
}

/// Updating a directory is a 409: a file never replaces a collection.
#[tokio::test]
async fn update_directory_target_conflicts() {
    let f = TestFixture::new().await;
    let dir = f.client.create_dir(&f.api_token, &f.repo_id, "/d").await;
    assert!(dir.status().is_success(), "creating the directory failed");

    let api = update_link(&f, "/").await;
    let resp = f
        .client
        .post_multipart_url(&api, form(&[("target_file", "/d")], &[("d", b"x")]))
        .await;
    assert_eq!(resp.status(), 409);
}

/// `update-blks-api` only accepts the commit phase (`commitonly`); a block-only
/// body is a 400, like the Go fileserver's "Only commit supported".
#[tokio::test]
async fn update_blks_requires_commitonly() {
    let f = TestFixture::new().await;
    let resp = f.client.update_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let url: String = resp.json().await.unwrap();

    // A valid block part proves the rejection is the commit-phase rule, not a
    // bad block id: the block is well-formed and would be stored.
    let block_id = infra::crypto::fs_id::sha1_hex(b"valid block");
    let resp = f
        .client
        .post_multipart_url(&url, form(&[], &[(block_id.as_str(), b"valid block")]))
        .await;
    assert_eq!(resp.status(), 400);
}

/// A block id is exactly 40 lowercase hex characters; uppercase is rejected
/// rather than stored under an id that can never resolve.
#[tokio::test]
async fn block_ids_reject_uppercase() {
    let f = TestFixture::new().await;
    let resp = f.client.update_blks_link(&f.api_token, &f.repo_id).await;
    let url: String = resp.json::<String>().await.unwrap();

    let upper = "A".repeat(40);
    let resp = f
        .client
        .post_multipart_url(
            &format!("{url}?commitonly=true"),
            form(
                &[
                    ("target_file", "/a.txt"),
                    ("blockids", &format!("[\"{upper}\"]")),
                    ("file_size", "0"),
                ],
                &[],
            ),
        )
        .await;
    assert_eq!(resp.status(), 400, "an uppercase block id must be rejected");
}

/// The raw-block phase answers `{"success":true}`; the iOS chain ignores the body
/// but the shape is part of the contract.
#[tokio::test]
async fn raw_blks_answers_success() {
    let f = TestFixture::new().await;
    let content = b"raw block payload";
    let block_id = infra::crypto::fs_id::sha1_hex(content);

    // seahub's POST handshake: the client names its blocks, the answer carries
    // the raw and commit URLs plus what is missing.
    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/upload-blks-link/?p=/", f.repo_id),
            Some(&f.api_token),
            &[("blklist", block_id.as_str())],
        )
        .await;
    assert_eq!(resp.status(), 200, "upload-blks-link POST failed");
    let link: serde_json::Value = resp.json().await.unwrap();
    let raw = link["rawblksurl"].as_str().unwrap().to_string();

    let resp = f
        .client
        .post_multipart_url(&raw, form(&[], &[(block_id.as_str(), content)]))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true, "got {body}");
}

/// `/api2/repos/{id}/file/` streams one multipart file, rejects `Content-Range`
/// (chunking belongs to the token endpoints) and requires the `file` part.
#[tokio::test]
async fn v1_file_upload_streams_and_rejects_chunks() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "v1.txt", b"v1 body")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        download(&f, "/v1.txt").await.as_deref(),
        Some(&b"v1 body"[..])
    );

    // A chunked request here would commit one chunk as the whole file, so the
    // endpoint rejects the header outright.
    let bare = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = bare
        .post(format!(
            "{}/api2/repos/{}/file/?p=/chunk.txt",
            f.server.base_url, f.repo_id
        ))
        .bearer_auth(&f.api_token)
        .header("content-range", "bytes 0-0/1")
        .multipart(form(&[], &[("chunk.txt", b"x")]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "Content-Range must be rejected");

    // A multipart body with fields but no `file` part: what a client that lost
    // the file sends.
    let resp = f
        .client
        .post_multipart(
            &format!("/api2/repos/{}/file/?p=/nofile.txt", f.repo_id),
            Some(&f.api_token),
            form(&[("parent_dir", "/")], &[]),
        )
        .await;
    let status = resp.status();
    let body = resp.text().await.unwrap();
    assert_eq!(
        status, 400,
        "a missing file part must be a 400, body: {body}"
    );

    // A `multipart/form-data` body without a boundary is malformed; it must not
    // surface as a 500 from the boundary parser.
    let resp = bare
        .post(format!(
            "{}/api2/repos/{}/file/?p=/noboundary.txt",
            f.server.base_url, f.repo_id
        ))
        .bearer_auth(&f.api_token)
        .header("content-type", "multipart/form-data")
        .body("not a multipart body")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "a missing boundary must be a 400");
}

/// Number of block files stored for one repository (the repo marker excluded).
///
/// The upload guard deletes a failed request's blocks asynchronously, so tests
/// poll this until the count settles.
fn block_file_count(block_root: &std::path::Path, repo_id: &str) -> usize {
    fn count(dir: &std::path::Path) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut files = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files += count(&path);
            } else if entry.file_name() != ".repo_id" {
                files += 1;
            }
        }
        files
    }
    let repo_dir = block_root
        .join(infra::storage::block_store::REPOS_DIR)
        .join(infra::crypto::fs_id::sha1_hex(repo_id.as_bytes()));
    count(&repo_dir)
}

/// A request rejected *after* its file part was streamed into the block store
/// (here: a `relative_path` that escapes the library) must not leave those
/// blocks behind. They used to stay unreferenced until the 48-hour pending-block
/// sweep, so an upload-link holder could fill the data disk with requests that
/// never commit.
#[tokio::test]
async fn a_rejected_target_does_not_leave_streamed_blocks() {
    let f = TestFixture::new().await;
    let url = upload_link(&f, "/", false).await;
    let before = block_file_count(&f.server.block_dir, &f.repo_id);

    // Long enough to produce at least one CDC block, so the guard has something
    // real to delete.
    let payload = vec![0xA5u8; 2 * 1024 * 1024];
    let resp = f
        .client
        .post_multipart_url(
            &url,
            form(
                &[("parent_dir", "/"), ("relative_path", "../../escape")],
                &[("escape.bin", &payload)],
            ),
        )
        .await;
    assert_eq!(
        resp.status(),
        400,
        "an escaping relative_path must be rejected"
    );

    // Cleanup runs in a spawned task after the response; give it a moment to
    // finish rather than sleeping a fixed time.
    let mut after = block_file_count(&f.server.block_dir, &f.repo_id);
    for _ in 0..60 {
        if after <= before {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        after = block_file_count(&f.server.block_dir, &f.repo_id);
    }
    assert!(
        after <= before,
        "a failed upload left {} unreferenced block(s) behind",
        after - before
    );
}
