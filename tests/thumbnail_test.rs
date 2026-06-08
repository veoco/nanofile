mod common;

use common::TestFixture;

/// B.4.1 — GET /api2/repos/{repo_id}/thumbnail/?p=&size=
#[tokio::test]
async fn test_thumbnail_not_found() {
    let f = TestFixture::new().await;

    // Upload a file (not an image) — thumbnail won't exist
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "test.txt", b"hello")
        .await;
    assert!(resp.status().is_success());

    // Request thumbnail for the text file — should 404 (can't generate thumbnail for text)
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/thumbnail/?p=/test.txt&size=48", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_thumbnail_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client
        .get("/api2/repos/some-repo/thumbnail/?p=/f.txt&size=48", None)
        .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_thumbnail_directory_returns_400() {
    let f = TestFixture::new().await;

    let resp = f.client.create_dir(&f.api_token, &f.repo_id, "/pics").await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/thumbnail/?p=/pics&size=48", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 400);
}
