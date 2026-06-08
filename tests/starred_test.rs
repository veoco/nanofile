mod common;

use common::TestFixture;

#[tokio::test]
async fn test_starred_files_empty() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get("/api2/starredfiles/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_starred_files_unauthorized() {
    let f = TestFixture::new().await;

    let resp = f.client.get("/api2/starredfiles/", None).await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_v21_starred_items_star_and_unstar() {
    let f = TestFixture::new().await;

    // Upload a file first
    let resp = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "star-me.txt",
            b"star content",
        )
        .await;
    assert!(resp.status().is_success(), "upload failed");

    // Star it via v2.1
    let resp = f
        .client
        .post_json(
            "/api/v2.1/starred-items/",
            Some(&f.api_token),
            &serde_json::json!({"repo_id": f.repo_id, "path": "/star-me.txt"}),
        )
        .await;
    assert_eq!(resp.status(), 200, "star failed: {:?}", resp.text().await);

    // List starred via v2.1
    let resp = f
        .client
        .get("/api/v2.1/starred-items/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["starred_item_list"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["repo_id"], f.repo_id);
    assert_eq!(items[0]["path"], "/star-me.txt");

    // Legacy API should also show it
    let resp = f
        .client
        .get("/api2/starredfiles/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let legacy: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(legacy.as_array().unwrap().len(), 1);

    // Unstar via v2.1 (DELETE with query params, path must be URL-encoded)
    let resp = f
        .client
        .delete(
            &format!(
                "/api/v2.1/starred-items/?repo_id={}&path=%2Fstar-me.txt",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // List should be empty now
    let resp = f
        .client
        .get("/api/v2.1/starred-items/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["starred_item_list"].as_array().unwrap().len(), 0);
}
