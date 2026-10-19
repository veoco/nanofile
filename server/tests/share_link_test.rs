mod common;

use common::{AuthLimits, TestFixture, create_test_user};

#[tokio::test]
async fn test_share_link_create_and_download() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "shared.txt",
            b"share content",
        )
        .await;
    assert!(up.status().is_success(), "upload failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/shared.txt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty(), "token must not be empty");

    let dl = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
    assert_eq!(dl.status(), 200, "shared download should succeed");
    let content = dl.bytes().await.unwrap();
    assert_eq!(&content[..], b"share content");
}

/// H.1a — Range requests on /f/{token}/?dl=1 (resumable shared download).
#[tokio::test]
async fn test_share_link_download_range() {
    let f = TestFixture::new().await;
    let content: Vec<u8> = (0..50u8).collect();

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "shared.bin", &content)
        .await;
    assert!(up.status().is_success(), "upload failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/shared.bin" }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let dl = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
    assert_eq!(dl.status(), 200);
    let headers = dl.headers().clone();
    assert_eq!(
        headers.get("content-length").and_then(|v| v.to_str().ok()),
        Some("50")
    );
    assert_eq!(
        headers.get("accept-ranges").and_then(|v| v.to_str().ok()),
        Some("bytes")
    );

    let dl = f
        .client
        .get_with_range(&format!("/f/{}/?dl=1", token), None, "bytes=20-29")
        .await;
    assert_eq!(dl.status(), 206);
    let headers = dl.headers().clone();
    assert_eq!(
        headers.get("content-range").and_then(|v| v.to_str().ok()),
        Some("bytes 20-29/50")
    );
    let body = dl.bytes().await.unwrap();
    assert_eq!(&body[..], &content[20..30]);
}

/// H.2 — Share link with password protection
#[tokio::test]
async fn test_share_link_with_password() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "secret.txt", b"secret data")
        .await;
    assert!(up.status().is_success(), "upload failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/secret.txt",
                "password": "mypassword",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let dl = f.client.get(&format!("/f/{}/", token), None).await;
    assert_eq!(dl.status(), 200, "must return 200 with password form");
    let body = dl.text().await.unwrap();
    assert!(
        body.contains("Password Required"),
        "should show password form"
    );

    // The password is never taken from the query string: `?password=` is
    // ignored, so it cannot grant access (and never lands in URLs or logs).
    let ignored = f
        .client
        .get(&format!("/f/{}/?dl=1&password=mypassword", token), None)
        .await;
    assert_eq!(
        ignored.status(),
        200,
        "ignoring the query parameter still renders the form"
    );
    let ignored_body = ignored.text().await.unwrap();
    assert!(
        ignored_body.contains("Password Required"),
        "a query-string password must not unlock the link"
    );

    let wrong = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}/f/{}/", f.server.base_url, token))
        .header("X-Seafile-Sharelink-Password", "wrongpass")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 200, "must return 200 with error");
    let body = wrong.text().await.unwrap();
    assert!(
        body.contains("Incorrect password"),
        "should show error message"
    );

    let raw_client = reqwest::Client::builder().no_proxy().build().unwrap();
    let ok = raw_client
        .get(format!("{}/f/{}/?dl=1", f.server.base_url, token))
        .header("X-Seafile-Sharelink-Password", "mypassword")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200, "should succeed with correct password");
    let content = ok.bytes().await.unwrap();
    assert_eq!(&content[..], b"secret data");
}

/// The password form must unlock via a signed cookie, and the redirect must not
/// carry the password in the URL (that is what used to leak it into browser
/// history, referrers and access logs).
#[tokio::test]
async fn test_share_link_password_form_sets_cookie_not_url() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "cookie.txt", b"secret data")
        .await;
    assert!(up.status().is_success(), "upload failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/cookie.txt",
                "password": "mypassword",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();

    let post = browser
        .post(format!("{}/f/{}/", f.server.base_url, token))
        .form(&[("password", "mypassword")])
        .send()
        .await
        .unwrap();
    assert_eq!(post.status(), 302, "successful password POST redirects");

    let location = post
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        !location.contains("password"),
        "redirect must not carry the password: {location}"
    );
    assert!(
        post.headers().get(reqwest::header::SET_COOKIE).is_some(),
        "the unlock must be handed over as a cookie"
    );

    let dl = browser
        .get(format!("{}/f/{}/?dl=1", f.server.base_url, token))
        .send()
        .await
        .unwrap();
    assert_eq!(dl.status(), 200, "cookie must unlock the link");
    assert_eq!(&dl.bytes().await.unwrap()[..], b"secret data");
}

/// Security: password attempts on a share link are rate limited per IP.
#[tokio::test]
async fn test_share_link_password_rate_limited() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "secret.txt", b"secret data")
        .await;
    assert!(up.status().is_success(), "upload failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/secret.txt",
                "password": "mypassword",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // link_password_max_per_hour = 10 in the test config; the 11th attempt
    // should be rejected with 429.
    for _ in 0..10 {
        let resp = f
            .client
            .post_form(
                &format!("/f/{}/", token),
                None,
                &[("password", "wrongpass")],
            )
            .await;
        assert_ne!(resp.status(), 429, "should not be rate limited yet");
    }
    let resp = f
        .client
        .post_form(
            &format!("/f/{}/", token),
            None,
            &[("password", "wrongpass")],
        )
        .await;
    assert_eq!(
        resp.status(),
        429,
        "should be rate limited after exceeding the threshold"
    );
}

/// Security: failed password attempts on the GET preview endpoint (which has
/// no IP limiter) are capped per link token, so distributed-IP brute force
/// against a single link is throttled. A correct password still works after
/// the failed attempts (successful traffic is never throttled).
#[tokio::test]
async fn test_share_link_password_limited_by_token_on_get() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "secret.txt", b"secret data")
        .await;
    assert!(up.status().is_success(), "upload failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/secret.txt",
                "password": "mypassword",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // link_password_max_per_hour = 10 in the test config; the 11th failed GET
    // attempt should be rejected with 429.
    let raw_client = reqwest::Client::builder().no_proxy().build().unwrap();
    for _ in 0..10 {
        let resp = raw_client
            .get(format!("{}/f/{}/", f.server.base_url, token))
            .header("X-Seafile-Sharelink-Password", "wrongpass")
            .send()
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            429,
            "should not be rate limited before exceeding the threshold"
        );
    }
    let resp = raw_client
        .get(format!("{}/f/{}/", f.server.base_url, token))
        .header("X-Seafile-Sharelink-Password", "wrongpass")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        429,
        "GET preview should be rate limited per token after failed attempts"
    );

    // A correct password still works — successful requests must not be
    // throttled even after the earlier failures.
    let ok = raw_client
        .get(format!("{}/f/{}/?dl=1", f.server.base_url, token))
        .header("X-Seafile-Sharelink-Password", "mypassword")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200, "correct password must still succeed");
}

/// H.3 — Share link with expiry (past timestamp)
#[tokio::test]
async fn test_share_link_expired() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "temp.txt", b"temporary")
        .await;
    assert!(up.status().is_success());

    // Create share link with negative expire_days (past expiry)
    // expire_days=-1 → now - 86400, always expired
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/temp.txt",
                "expire_days": -1,
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let dl = f.client.get(&format!("/f/{}/", token), None).await;
    assert_eq!(dl.status(), 404, "expired link must return 404");
}

/// H.4 — v2.1 DELETE share link by creator should succeed
#[tokio::test]
async fn test_share_link_delete_v21_own() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "delete_test.txt",
            b"delete test",
        )
        .await;
    assert!(up.status().is_success(), "upload failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/delete_test.txt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let dl = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
    assert_eq!(dl.status(), 200, "download should work before delete");

    let del = f
        .client
        .delete(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        del.status(),
        200,
        "creator should be able to delete own link"
    );

    let dl = f.client.get(&format!("/f/{}/", token), None).await;
    assert_eq!(dl.status(), 404);
}

/// H.5 — v2.1 DELETE share link by other user should fail
#[tokio::test]
async fn test_share_link_delete_v21_other() {
    let f = TestFixture::new().await;
    let db = &*f.server.db;

    let up = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "other_test.txt",
            b"other test",
        )
        .await;
    assert!(up.status().is_success(), "upload failed");

    let _user2_id = create_test_user(db, "user2@test.com", "password2").await;
    let resp2 = f.client.login("user2@test.com", "password2").await;
    assert_eq!(resp2.status(), 200);
    let token2_val: serde_json::Value = resp2.json().await.unwrap();
    let api_token2 = token2_val["token"].as_str().unwrap().to_string();

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/other_test.txt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let del = f
        .client
        .delete(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&api_token2),
        )
        .await;
    assert_eq!(del.status(), 404, "other user's delete must return 404");

    let dl = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
    assert_eq!(dl.status(), 200, "link should still be valid");
}

/// H.6 — v1 API (api2/shared-links) DELETE by creator should succeed
#[tokio::test]
async fn test_share_link_delete_v1_own() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "v1delete.txt", b"v1 test")
        .await;
    assert!(up.status().is_success(), "upload failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/v1delete.txt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link via v2.1 failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty());

    let del = f
        .client
        .delete(&format!("/api2/shared-links/{}", token), Some(&f.api_token))
        .await;
    assert_eq!(del.status(), 200, "v1 delete should succeed for creator");
}

/// H.7 — Encrypted repo blocks share link creation
#[tokio::test]
async fn test_share_link_encrypted_repo_blocked() {
    let f = TestFixture::new().await;

    let repo_id = uuid::Uuid::new_v4().to_string();
    let magic = infra::crypto::key_derivation::generate_magic(&repo_id, "testpass", 2, "").unwrap();
    let random_key =
        infra::crypto::key_derivation::generate_random_key_for_repo("testpass", 2, "").unwrap();

    let resp = f
        .client
        .create_encrypted_repo(&f.api_token, "enc-repo", &repo_id, &magic, &random_key, 2)
        .await;
    assert_eq!(resp.status(), 201, "create encrypted repo failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(
        resp.status(),
        400,
        "must reject share link for encrypted repo"
    );
}

/// H.8 — Invalid token returns 404
#[tokio::test]
async fn test_share_link_invalid_token() {
    let f = TestFixture::new().await;
    let dl = f.client.get("/f/nonexistent-token-xxx/", None).await;
    assert_eq!(dl.status(), 404);
}

/// H.9 — Share link list returns expected fields
#[tokio::test]
async fn test_share_link_list_response_fields() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let list = f
        .client
        .get("/api/v2.1/share-links/", Some(&f.api_token))
        .await;
    assert_eq!(list.status(), 200);
    let body: serde_json::Value = list.json().await.unwrap();
    let links = body.as_array().unwrap();
    assert!(!links.is_empty(), "should have at least one share link");

    let link = &links[0];
    assert!(
        !link["token"].as_str().unwrap_or("").is_empty(),
        "token missing"
    );
    let link_url = link["link"].as_str().unwrap_or("");
    // `link` must be an absolute URL (Android client shares it verbatim).
    assert!(
        link_url.starts_with("http://") || link_url.starts_with("https://"),
        "link should be an absolute URL, got: {link_url}"
    );
    let token = link["token"].as_str().unwrap();
    assert!(
        link_url.contains(&format!("/f/{token}/")) || link_url.contains(&format!("/d/{token}/")),
        "link should contain /f/{{token}}/ or /d/{{token}}/ path"
    );
    assert!(
        link["has_password"].is_boolean(),
        "has_password should be boolean"
    );
    assert!(link["s_type"].is_string(), "s_type should be string");
    assert_eq!(link["view_cnt"], 0, "view_cnt should be 0");
    assert!(
        link.get("expire_at").is_some(),
        "expire_at should be present"
    );
}

/// H.10 — Upload link creation with password
#[tokio::test]
async fn test_upload_link_create_with_password() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
                "password": "uploadpass",
            }),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "create upload link with password failed"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty());

    let list = f
        .client
        .get("/api/v2.1/upload-links/", Some(&f.api_token))
        .await;
    assert_eq!(list.status(), 200);
    let list_body: serde_json::Value = list.json().await.unwrap();
    let links = list_body.as_array().unwrap();
    let ul = links.iter().find(|l| l["token"] == token).unwrap();
    assert_eq!(
        ul["has_password"], true,
        "upload link should have has_password=true"
    );
}

/// H.11 — PUT update password on share link
#[tokio::test]
async fn test_share_link_update_password() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "pw_update.txt", b"pw update")
        .await;
    assert!(up.status().is_success());

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/pw_update.txt",
                "password": "oldpass",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let raw_client = reqwest::Client::builder().no_proxy().build().unwrap();
    let ok = raw_client
        .get(format!("{}/f/{}/?dl=1", f.server.base_url, token))
        .header("X-Seafile-Sharelink-Password", "oldpass")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&f.api_token),
            &serde_json::json!({"password": "newpass"}),
        )
        .await;
    assert_eq!(upd.status(), 200, "update password should succeed");

    let old = raw_client
        .get(format!("{}/f/{}/?dl=1", f.server.base_url, token))
        .header("X-Seafile-Sharelink-Password", "oldpass")
        .send()
        .await
        .unwrap();
    assert_eq!(old.status(), 200, "wrong password shows form");
    let body_text = old.text().await.unwrap();
    assert!(
        body_text.contains("Incorrect password"),
        "old password should be rejected"
    );

    let ok = raw_client
        .get(format!("{}/f/{}/?dl=1", f.server.base_url, token))
        .header("X-Seafile-Sharelink-Password", "newpass")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    let content = ok.bytes().await.unwrap();
    assert_eq!(&content[..], b"pw update");
}

#[tokio::test]
async fn test_share_link_clear_password() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "pw_clear.txt", b"pw clear")
        .await;
    assert!(up.status().is_success());

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/pw_clear.txt",
                "password": "secret",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&f.api_token),
            &serde_json::json!({"password": null}),
        )
        .await;
    assert_eq!(upd.status(), 200, "clear password should succeed");

    let ok = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
    assert_eq!(ok.status(), 200);
    let content = ok.bytes().await.unwrap();
    assert_eq!(&content[..], b"pw clear");
}

/// H.13 — PUT update expiry on share link
#[tokio::test]
async fn test_share_link_update_expiry() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "exp_test.txt", b"exp test")
        .await;
    assert!(up.status().is_success());

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/exp_test.txt",
                "expire_days": 1,
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let ok = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
    assert_eq!(ok.status(), 200);

    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&f.api_token),
            &serde_json::json!({"expire_days": null}),
        )
        .await;
    assert_eq!(upd.status(), 200, "clear expiry should succeed");
}

#[tokio::test]
async fn test_share_link_update_other_fails() {
    let f = TestFixture::new().await;
    let db = &*f.server.db;

    let up = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "other_update.txt",
            b"other update",
        )
        .await;
    assert!(up.status().is_success());

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/other_update.txt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let _user2_id = common::create_test_user(db, "user2-upd@test.com", "password2").await;
    let resp2 = f.client.login("user2-upd@test.com", "password2").await;
    assert_eq!(resp2.status(), 200);
    let token2_val: serde_json::Value = resp2.json().await.unwrap();
    let api_token2 = token2_val["token"].as_str().unwrap().to_string();

    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&api_token2),
            &serde_json::json!({"expire_days": 7}),
        )
        .await;
    assert_eq!(upd.status(), 404, "non-owner update must return 404");
}

#[tokio::test]
async fn test_share_link_update_nonexistent() {
    let f = TestFixture::new().await;

    let upd = f
        .client
        .put_json(
            "/api/v2.1/share-links/nonexistent-token/",
            Some(&f.api_token),
            &serde_json::json!({"expire_days": 7}),
        )
        .await;
    assert_eq!(upd.status(), 404, "non-existent token must return 404");
}

#[tokio::test]
async fn test_share_link_create_form_encoded_desktop() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "desktop_form.txt",
            b"desktop form",
        )
        .await;
    assert!(up.status().is_success(), "upload failed");

    // The desktop client sends application/x-www-form-urlencoded.
    let resp = f
        .client
        .post_form(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &[
                ("repo_id", &f.repo_id),
                ("path", "/desktop_form.txt"),
                ("expiration_time", "2099-12-31T23:59:59+08:00"),
            ],
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "form-encoded share link create failed: {:?}",
        resp.text().await
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();
    assert!(!token.is_empty());
    // The response link must be an absolute URL.
    let link = body["link"].as_str().unwrap_or("");
    assert!(
        link.starts_with("http://") || link.starts_with("https://"),
        "link should be absolute, got: {link}"
    );
    // expiration_time (year 2099) must be honoured.
    let expire = body["expire_date"].as_i64().unwrap_or(0);
    assert!(expire > 0, "expire_date should be set from expiration_time");

    // The shared link should be downloadable.
    let dl = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
    assert_eq!(dl.status(), 200, "shared download should succeed");
}

/// H.1c — POST /api/v2.1/share-links/ for a directory returns `s_type: "d"`
/// and a `/d/` link, so the Web UI builds the right URL.
/// Regression test: the create response previously omitted `s_type`, making the
/// frontend fall back to the row's `data-type` ("dir") and build a `/f/` link.
#[tokio::test]
async fn test_share_link_create_directory_returns_d_type() {
    let f = TestFixture::new().await;

    let mkdir = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/shared-dir")
        .await;
    assert!(mkdir.status().is_success(), "mkdir failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/shared-dir" }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    assert_eq!(
        body["s_type"], "d",
        "directory share link must expose s_type=\"d\""
    );
    let link = body["link"].as_str().unwrap_or("");
    assert!(
        link.contains(&format!("/d/{}/", token)),
        "directory share link URL should be /d/..., got: {link}"
    );
}

/// H.1d — GET /d/{token}/ paginates a large directory (200 entries per page)
/// instead of rendering the whole folder in one response.
#[tokio::test]
async fn test_share_dir_view_paginates_large_folder() {
    let f = TestFixture::new().await;

    let mkdir = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/shared-dir")
        .await;
    assert!(mkdir.status().is_success(), "mkdir failed");

    for i in 0..250 {
        let up = f
            .client
            .upload_file(
                &f.api_token,
                &f.repo_id,
                "/shared-dir",
                &format!("file-{i:03}.txt"),
                b"content",
            )
            .await;
        assert!(up.status().is_success(), "upload {i} failed");
    }

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/shared-dir" }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let page1 = f
        .client
        .get(&format!("/d/{}/", token), None)
        .await
        .text()
        .await
        .unwrap();
    assert_eq!(
        page1.matches("class=\"nf-prow-name\"").count(),
        200,
        "page 1 should render exactly 200 entries"
    );
    assert!(page1.contains("page=2"), "page 1 should link to page 2");

    let page2 = f
        .client
        .get(&format!("/d/{}/?page=2", token), None)
        .await
        .text()
        .await
        .unwrap();
    assert_eq!(
        page2.matches("class=\"nf-prow-name\"").count(),
        50,
        "page 2 should render the remaining 50 entries"
    );
    assert!(
        page2.contains("page=1"),
        "page 2 should link back to page 1"
    );
}

/// Create a second user, grant them `permission` on the repo, and return their
/// login token.
async fn shared_member_token(f: &TestFixture, email: &str, permission: &str) -> String {
    let uid = create_test_user(&f.server.db, email, "password").await;
    common::add_repo_member(&f.server.db, &f.repo_id, uid, permission).await;

    let resp = f.client.login(email, "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    body["token"].as_str().unwrap().to_string()
}

/// A read-only shared member must not be able to enumerate the owner's
/// share-link tokens via the repo-filtered listing (tokens are unauthenticated
/// bearer URLs).
#[tokio::test]
async fn test_share_link_repo_filter_only_returns_own() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/" }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let ro_token = shared_member_token(&f, "readonly-links@test.com", "r").await;

    let list = f
        .client
        .get(
            &format!("/api/v2.1/share-links/?repo_id={}&path=/", f.repo_id),
            Some(&ro_token),
        )
        .await;
    assert_eq!(list.status(), 200);
    let links = list.json::<serde_json::Value>().await.unwrap();
    assert_eq!(links.as_array().unwrap().len(), 0);

    let list = f
        .client
        .get(
            &format!("/api/v2.1/share-links/?repo_id={}&path=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(list.status(), 200);
    let links = list.json::<serde_json::Value>().await.unwrap();
    assert_eq!(links.as_array().unwrap().len(), 1);
}

/// A read-only shared member must not be able to enumerate the owner's
/// upload-link tokens via the repo+path filtered listing (an upload token
/// grants anonymous write access).
#[tokio::test]
async fn test_upload_link_repo_filter_only_returns_own() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/" }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let ro_token = shared_member_token(&f, "readonly-ul@test.com", "r").await;

    let list = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/?repo_id={}&path=/", f.repo_id),
            Some(&ro_token),
        )
        .await;
    assert_eq!(list.status(), 200);
    let links = list.json::<serde_json::Value>().await.unwrap();
    assert_eq!(links.as_array().unwrap().len(), 0);

    let list = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/?repo_id={}&path=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let links = list.json::<serde_json::Value>().await.unwrap();
    assert_eq!(links.as_array().unwrap().len(), 1);
}

/// The repo-scoped upload-links endpoint must only return the caller's own
/// upload links, never other members'.
#[tokio::test]
async fn test_repo_upload_links_only_returns_own() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/" }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let ro_token = shared_member_token(&f, "readonly-rul@test.com", "r").await;

    let list = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/upload-links/", f.repo_id),
            Some(&ro_token),
        )
        .await;
    assert_eq!(list.status(), 200);
    let links = list.json::<serde_json::Value>().await.unwrap();
    assert_eq!(links.as_array().unwrap().len(), 0);

    let list = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/upload-links/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let links = list.json::<serde_json::Value>().await.unwrap();
    assert_eq!(links.as_array().unwrap().len(), 1);
}

/// A read-write member who creates their own links sees only those, never the
/// owner's.
#[tokio::test]
async fn test_rw_member_sees_only_own_links() {
    let f = TestFixture::new().await;

    let rw_token = shared_member_token(&f, "rw-links@test.com", "rw").await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/" }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&rw_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/" }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&rw_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/" }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let list = f
        .client
        .get(
            &format!("/api/v2.1/share-links/?repo_id={}&path=/", f.repo_id),
            Some(&rw_token),
        )
        .await;
    let links = list.json::<serde_json::Value>().await.unwrap();
    assert_eq!(links.as_array().unwrap().len(), 1);

    let list = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/upload-links/", f.repo_id),
            Some(&rw_token),
        )
        .await;
    let links = list.json::<serde_json::Value>().await.unwrap();
    assert_eq!(links.as_array().unwrap().len(), 1);
}

/// Anonymous share-link downloads are rate-limited per client IP: requests
/// beyond `share_download_max_per_minute` within the window get 429.
#[tokio::test]
async fn test_share_download_rate_limited() {
    let f = TestFixture::new_with_auth_limits(AuthLimits {
        share_download_max_per_minute: 3,
    })
    .await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "rate.txt", b"data")
        .await;
    assert!(up.status().is_success(), "upload failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/rate.txt" }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    for i in 0..3 {
        let dl = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
        assert_eq!(dl.status(), 200, "download {i} should succeed");
    }
    let dl = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
    assert_eq!(
        dl.status(),
        429,
        "download beyond the per-minute cap should be rate limited"
    );
}

/// With both halves of sharing off, creating share or upload links is rejected
/// with 403 and the server advertises the `share-link-disabled` feature so
/// clients hide sharing UI.
#[tokio::test]
async fn test_sharing_disabled_globally() {
    let f = TestFixture::new_with_sharing_disabled().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "disabled.txt", b"data")
        .await;
    assert!(up.status().is_success(), "upload failed");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/disabled.txt" }),
        )
        .await;
    assert_eq!(
        resp.status(),
        403,
        "share-link creation should be forbidden"
    );

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/" }),
        )
        .await;
    assert_eq!(
        resp.status(),
        403,
        "upload-link creation should be forbidden"
    );

    let resp = f.client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let features = body["features"].as_array().unwrap();
    assert!(
        features.iter().any(|f| f == "share-link-disabled"),
        "share-link-disabled feature should be advertised"
    );
}

/// A share link that already existed before download sharing was turned off
/// answers as gone: `/d/{token}/` renders the dead-link page, 404.
#[tokio::test]
async fn test_share_link_disabled_blocks_existing_links() {
    let f = TestFixture::new_with_share_download_disabled().await;

    // Insert a share link directly (as if created before the switch was off).
    let token = "pre-existing-token".to_string();
    let now = chrono::Utc::now().timestamp();
    let model = infra::entity::share_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(f.repo_id.clone()),
        creator_id: sea_orm::Set(f.user_id),
        path: sea_orm::Set("/".to_string()),
        token: sea_orm::Set(token.clone()),
        password: sea_orm::Set(None),
        expires_at: sea_orm::Set(None),
        created_at: sea_orm::Set(now),
        s_type: sea_orm::Set("d".to_string()),
        view_cnt: sea_orm::Set(0i64),
        description: sea_orm::Set(None),
    };
    use infra::entity::share_link;
    use sea_orm::EntityTrait;
    share_link::Entity::insert(model)
        .exec(&*f.server.db)
        .await
        .expect("failed to insert pre-existing share link");

    let resp = f.client.get(&format!("/d/{}/", token), None).await;
    assert_eq!(
        resp.status(),
        404,
        "an existing share link must read as gone, not forbidden"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("Link unavailable"),
        "the dead-link page explains itself, got: {body}"
    );
}

/// Deactivating the account that created a link revokes the link too, through
/// the same creator-access check (here via the admin service, which is what the
/// sysadmin UI and API call).
#[tokio::test]
async fn test_links_are_revoked_when_the_creator_is_deactivated() {
    let f = TestFixture::new().await;
    assert!(
        f.client
            .upload_file(&f.api_token, &f.repo_id, "/", "deactivated.txt", b"x")
            .await
            .status()
            .is_success()
    );

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({"repo_id": f.repo_id, "path": "/deactivated.txt"}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        f.client
            .get(&format!("/f/{}/?dl=1", token), None)
            .await
            .status(),
        200
    );

    let svc = server::service::admin::AdminUserService::new(f.server.repos.clone());
    svc.update_user(f.user_id, false, false, None)
        .await
        .expect("deactivation succeeds");

    assert_eq!(
        f.client
            .get(&format!("/f/{}/?dl=1", token), None)
            .await
            .status(),
        404,
        "a deactivated account's links must stop resolving"
    );
}
