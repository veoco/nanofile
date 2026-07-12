#![allow(dead_code)]

mod common;

use common::{TestServer, create_test_admin, create_test_user};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn test_api_login_records_last_login() {
    let server = TestServer::start().await;
    let user_id = create_test_user(&server.db, "api-login@test.com", "password").await;
    let client = server.client();

    // Verify last_login_at is None before login.
    let user_before = infra::entity::user::Entity::find_by_id(user_id)
        .one(&*server.db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        user_before.last_login_at.is_none(),
        "last_login_at should be None before first login"
    );

    // Login via API.
    let resp = client.login("api-login@test.com", "password").await;
    assert_eq!(resp.status(), 200, "API login should succeed");

    // Verify last_login_at is now set.
    let user_after = infra::entity::user::Entity::find_by_id(user_id)
        .one(&*server.db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        user_after.last_login_at.is_some(),
        "last_login_at should be set after API login"
    );
}

#[tokio::test]
async fn test_web_login_records_last_login() {
    let server = TestServer::start().await;
    let user_id = create_test_user(&server.db, "web-login@test.com", "password").await;
    let db = &*server.db;

    // Login via Web UI form (use no-redirect client to catch the 302).
    let client = no_redirect_client();
    let base_url = &server.base_url;
    let resp = client
        .post(format!("{}/accounts/login/", base_url))
        .form(&[("email", "web-login@test.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        302,
        "Web UI login should redirect (302), got {}",
        resp.status()
    );

    // Verify last_login_at is now set.
    let user_after = infra::entity::user::Entity::find_by_id(user_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        user_after.last_login_at.is_some(),
        "last_login_at should be set after Web UI login"
    );
}

#[tokio::test]
async fn test_registration_records_last_login() {
    let server = TestServer::start().await;
    let db = &*server.db;

    // Create an admin user to own the invitation code.
    let admin_id = create_test_admin(db, "admin@test.com", "adminpass").await;

    // Directly insert an invitation code into the database.
    let now = chrono::Utc::now().timestamp();
    let code = "TEST-INVITE-CODE-123";
    let invite = infra::entity::invitation_code::ActiveModel {
        id: sea_orm::NotSet,
        code: Set(code.to_string()),
        email: Set(Some("newuser@test.com".to_string())),
        creator_id: Set(admin_id),
        created_at: Set(now),
        used_by: Set(None),
        used_at: Set(None),
    };
    invite.insert(db).await.unwrap();

    // Register with the invitation code (no-redirect client to catch 302).
    let client = no_redirect_client();
    let base_url = &server.base_url;
    let reg_resp = client
        .post(format!("{}/accounts/register/", base_url))
        .form(&[
            ("email", "newuser@test.com"),
            ("password1", "StrongP@ss1"),
            ("password2", "StrongP@ss1"),
            ("invitation_code", code),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(
        reg_resp.status(),
        302,
        "registration should redirect (302), got {}",
        reg_resp.status()
    );

    // Verify the new user has last_login_at set.
    let new_user = infra::entity::user::Entity::find()
        .filter(infra::entity::user::Column::Email.eq("newuser@test.com"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        new_user.last_login_at.is_some(),
        "last_login_at should be set after registration auto-login"
    );
}
