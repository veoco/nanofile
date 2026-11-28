//! User administration: the seahub `/api/v2.1/admin/users/` API, the password
//! policy and the web UI's own create form.

use crate::common;

use common::TestServer;
use serde_json::json;

async fn admin_token(server: &TestServer, email: &str, password: &str) -> String {
    let resp = server.client().login(email, password).await;
    assert_eq!(resp.status(), 200, "admin login failed");
    resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .expect("login token")
        .to_string()
}

#[tokio::test]
async fn admin_api_create_user_rejects_a_password_below_the_policy() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let token = admin_token(&server, "root@example.com", "password123").await;

    let weak = server
        .client()
        .post_json(
            "/api/v2.1/admin/users/",
            Some(&token),
            &json!({"email": "weak@example.com", "password": "short"}),
        )
        .await;
    assert_eq!(
        weak.status(),
        400,
        "the default minimum length is 8 characters"
    );

    let accepted = server
        .client()
        .post_json(
            "/api/v2.1/admin/users/",
            Some(&token),
            &json!({"email": "ok@example.com", "password": "password123"}),
        )
        .await;
    assert_eq!(
        accepted.status(),
        200,
        "a policy-compliant password must be accepted"
    );
}

/// The seahub-shaped surface: list envelope, email-addressed single user, add,
/// partial update and delete.
#[tokio::test]
async fn admin_api_user_crud_uses_the_seahub_shape() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let token = admin_token(&server, "root@example.com", "password123").await;
    let client = server.client();

    let created = client
        .post_json(
            "/api/v2.1/admin/users/",
            Some(&token),
            &json!({
                "email": "user@example.com",
                "password": "password123",
                "name": "User",
                "is_staff": false,
                "is_active": true,
            }),
        )
        .await;
    assert_eq!(created.status(), 200, "add user failed");
    let created: serde_json::Value = created.json().await.unwrap();
    assert_eq!(created["email"], "user@example.com");
    assert_eq!(created["name"], "User");
    assert_eq!(created["is_staff"], false);
    assert_eq!(created["is_active"], true);
    assert_eq!(created["role"], "default");
    assert!(
        created["create_time"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "create_time must be an ISO timestamp: {created}"
    );
    assert!(
        created["quota_total"].is_i64(),
        "quota_total is bytes, or -1 when unlimited: {created}"
    );

    let list = client.get("/api/v2.1/admin/users/", Some(&token)).await;
    assert_eq!(list.status(), 200, "list users failed");
    let list: serde_json::Value = list.json().await.unwrap();
    assert_eq!(list["total_count"], 2, "admin + created user");
    assert!(
        list["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["email"] == "user@example.com"),
        "the created user must be listed: {list}"
    );

    let single = client
        .get("/api/v2.1/admin/users/user@example.com/", Some(&token))
        .await;
    assert_eq!(single.status(), 200, "single user by email failed");

    let updated = client
        .put_json(
            "/api/v2.1/admin/users/user@example.com/",
            Some(&token),
            &json!({"is_active": false, "quota_total": 10, "name": "Renamed"}),
        )
        .await;
    assert_eq!(updated.status(), 200, "update failed");
    let updated: serde_json::Value = updated.json().await.unwrap();
    assert_eq!(updated["is_active"], false);
    assert_eq!(updated["name"], "Renamed");
    assert_eq!(
        updated["quota_total"],
        10 * 1024 * 1024,
        "quota_total is sent in MB and answered in bytes"
    );

    // A deactivated account cannot sign in any more.
    let login = client.login("user@example.com", "password123").await;
    assert_ne!(
        login.status(),
        200,
        "a deactivated account must not sign in"
    );

    let deleted = client
        .delete("/api/v2.1/admin/users/user@example.com/", Some(&token))
        .await;
    assert_eq!(deleted.status(), 200, "delete failed");
    let body: serde_json::Value = deleted.json().await.unwrap();
    assert_eq!(body["success"], true);

    let gone = client
        .get("/api/v2.1/admin/users/user@example.com/", Some(&token))
        .await;
    assert_eq!(gone.status(), 404, "the deleted user must be gone");
}

#[tokio::test]
async fn admin_api_rejects_non_admins_and_anonymous_callers() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    common::create_test_user(&server.db, "plain@example.com", "password123").await;
    let plain = admin_token(&server, "plain@example.com", "password123").await;
    let client = server.client();

    let denied = client.get("/api/v2.1/admin/users/", Some(&plain)).await;
    assert_eq!(denied.status(), 403, "a normal account is not an admin");

    let anonymous = client.get("/api/v2.1/admin/users/", None).await;
    assert_eq!(anonymous.status(), 401, "the API requires a credential");
}

#[tokio::test]
async fn sysadmin_create_user_rejects_a_password_below_the_policy() {
    let server = TestServer::start().await;
    common::create_test_admin(&server.db, "root@example.com", "password123").await;
    let client = common::web::ui_login(&server, "root@example.com", "password123").await;
    let (status, html) = common::web::page(&server, &client, "/sysadmin/users/").await;
    assert_eq!(status, 200);
    let csrf = common::web::csrf_of(&html);

    let resp = client
        .post(format!("{}/sysadmin/users/create/", server.base_url))
        .form(&[
            ("email", "weak@example.com"),
            ("password", "short"),
            ("csrf_token", csrf.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "the page re-renders with an error banner"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("Password must be at least 8 characters long."),
        "the banner must name the policy: {body}"
    );
}
