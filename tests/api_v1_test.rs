mod common;

use common::TestFixture;

/// F.1 — SDoc Comments
#[tokio::test]
async fn test_sdoc_comments_empty() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get("/api/v1/docs/some-uuid/comment/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
}

/// G.1 — Metadata config
#[tokio::test]
async fn test_metadata_config() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/metadata/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_metadata_tags() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/metadata/tags/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
}
