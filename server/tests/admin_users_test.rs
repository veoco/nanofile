//! User administration: the configured password policy and the legacy profile route.

use crate::common;

use common::TestServer;

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
            "/api/admin/users/",
            Some(&token),
            &serde_json::json!({"email": "weak@example.com", "password": "short"}),
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
            "/api/admin/users/",
            Some(&token),
            &serde_json::json!({"email": "ok@example.com", "password": "password123"}),
        )
        .await;
    assert_eq!(
        accepted.status(),
        200,
        "a policy-compliant password must be accepted"
    );
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
