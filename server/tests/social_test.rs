mod common;

use common::TestFixture;
use common::create_test_user;
use sea_orm::{ActiveModelTrait, DatabaseConnection};

/// Insert a group plus its members (used to exercise the v2.1 groups endpoint).
async fn insert_group_with_members(
    db: &DatabaseConnection,
    name: &str,
    creator_id: i32,
    members: &[(i32, &str)],
) -> i32 {
    let now = chrono::Utc::now().timestamp();
    let g = infra::entity::group::ActiveModel {
        id: sea_orm::NotSet,
        name: sea_orm::Set(name.to_string()),
        creator_id: sea_orm::Set(creator_id),
        created_at: sea_orm::Set(now),
    };
    let g = g.insert(db).await.unwrap();
    for (uid, role) in members {
        infra::entity::group_member::ActiveModel {
            id: sea_orm::NotSet,
            group_id: sea_orm::Set(g.id),
            user_id: sea_orm::Set(*uid),
            role: sea_orm::Set(role.to_string()),
            created_at: sea_orm::Set(now),
        }
        .insert(db)
        .await
        .unwrap();
    }
    g.id
}

/// B.6.1 — GET /api2/groups/
#[tokio::test]
async fn test_groups_empty() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/api2/groups/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_groups_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/api2/groups/", None).await;
    assert_eq!(resp.status(), 401);
}

/// B.6.2 — GET /api2/groupandcontacts/
#[tokio::test]
async fn test_groupandcontacts_empty() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get("/api2/groupandcontacts/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["groups"].as_array().unwrap().is_empty());
    assert!(body["contacts"].as_array().unwrap().is_empty());
}

/// B.6.3 — GET /api2/search-user/?q=
#[tokio::test]
async fn test_search_user_found() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get("/api2/search-user/?q=test", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_search_user_not_found() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            "/api2/search-user/?q=nonexistent_user_xyz",
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 0);
}

/// v2.1 — GET /api/v2.1/groups/ (required by seadroid `getGroupsAsync`, part of
/// the library-list load chain; must return 200 + array, never 404).
#[tokio::test]
async fn test_groups_v21_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/api/v2.1/groups/", None).await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_groups_v21_empty() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get("/api/v2.1/groups/?with_repos=0", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_groups_v21_lists_member_groups_with_official_fields() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let db = &*server.db;

    let owner_id = create_test_user(db, "owner@example.com", "password").await;
    let admin_id = create_test_user(db, "admin@example.com", "password").await;
    let member_id = create_test_user(db, "member@example.com", "password").await;

    let group_id = insert_group_with_members(
        db,
        "Dev Team",
        owner_id,
        &[
            (owner_id, "Owner"),
            (admin_id, "Admin"),
            (member_id, "Member"),
        ],
    )
    .await;

    let resp = client.login("owner@example.com", "password").await;
    assert_eq!(resp.status(), 200, "login failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let resp = client
        .get("/api/v2.1/groups/?with_repos=0", Some(&token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert_eq!(
        arr.len(),
        1,
        "expected exactly the one group the owner belongs to"
    );

    let group = &arr[0];
    assert_eq!(group["id"].as_i64(), Some(group_id as i64));
    assert_eq!(group["name"].as_str(), Some("Dev Team"));
    assert_eq!(group["owner"].as_str(), Some("owner@example.com"));
    assert_eq!(group["parent_group_id"].as_i64(), Some(0));
    assert_eq!(group["group_quota_usage"].as_i64(), Some(0));
    assert!(group["created_at"].is_string());
    assert!(!group["created_at"].as_str().unwrap().is_empty());

    let admins: Vec<&str> = group["admins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert!(
        admins.contains(&"owner@example.com"),
        "owner must be in admins"
    );
    assert!(
        admins.contains(&"admin@example.com"),
        "admin must be in admins"
    );
    assert!(
        !admins.contains(&"member@example.com"),
        "plain member is not an admin"
    );
}

#[tokio::test]
async fn test_groups_v21_member_only_sees_own_groups() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let db = &*server.db;

    let owner_id = create_test_user(db, "owner@example.com", "password").await;
    let other_id = create_test_user(db, "other@example.com", "password").await;
    let _outsider_id = create_test_user(db, "outsider@example.com", "password").await;

    insert_group_with_members(
        db,
        "Team A",
        owner_id,
        &[(owner_id, "Owner"), (other_id, "Member")],
    )
    .await;

    // outsider is not in any group
    let resp = client.login("outsider@example.com", "password").await;
    assert_eq!(resp.status(), 200, "login failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let resp = client.get("/api/v2.1/groups/", Some(&token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body.as_array().unwrap().len(),
        0,
        "non-member sees no groups"
    );
}

#[tokio::test]
async fn test_groups_v21_with_repos_returns_empty_repos() {
    let f = TestFixture::new().await;
    let server = &f.server;
    let db = &*server.db;
    let group_id =
        insert_group_with_members(db, "Team B", f.user_id, &[(f.user_id, "Owner")]).await;

    let resp = f
        .client
        .get("/api/v2.1/groups/?with_repos=1", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"].as_i64(), Some(group_id as i64));
    assert!(arr[0]["repos"].is_array());
    assert_eq!(arr[0]["repos"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_groups_v21_invalid_with_repos() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get("/api/v2.1/groups/?with_repos=2", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 400);
}
