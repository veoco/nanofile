//! Proof-of-work challenges on the anonymous web forms.

mod common;

use common::{TestServer, create_test_user};
use sha2::{Digest, Sha256};

/// The exact bytes both the browser and `PowStore` hash.
fn hash_input(seed: &[u8; 16], nonce: u64) -> [u8; 24] {
    let mut input = [0u8; 24];
    input[..16].copy_from_slice(seed);
    input[16..].copy_from_slice(&nonce.to_be_bytes());
    input
}

fn leading_zero_bits(hash: &[u8; 32]) -> u32 {
    let mut bits = 0;
    for byte in hash {
        if *byte == 0 {
            bits += 8;
        } else {
            bits += byte.leading_zeros();
            break;
        }
    }
    bits
}

fn solve(seed: &[u8; 16], bits: u8) -> String {
    for nonce in 0..=u32::MAX as u64 {
        let digest: [u8; 32] = Sha256::digest(hash_input(seed, nonce)).into();
        if leading_zero_bits(&digest) >= u32::from(bits) {
            return nonce.to_string();
        }
    }
    unreachable!("a 12-bit target always has a 32-bit solution")
}

fn attribute(html: &str, name: &str) -> String {
    let marker = format!(r#"{name}=""#);
    let start = html
        .find(&marker)
        .unwrap_or_else(|| panic!("missing {marker}"))
        + marker.len();
    let end = html[start..].find('"').expect("unterminated attribute");
    html[start..start + end].to_string()
}

fn hidden_value(html: &str, name: &str) -> String {
    let marker = format!(r#"name="{name}" value=""#);
    let start = html
        .find(&marker)
        .unwrap_or_else(|| panic!("missing {marker}"))
        + marker.len();
    let end = html[start..].find('"').expect("unterminated attribute");
    html[start..start + end].to_string()
}

fn challenge(html: &str) -> (String, [u8; 16], u8) {
    let id = hidden_value(html, "pow_id");
    let seed_bytes = hex::decode(attribute(html, "data-pow-seed")).expect("hex seed");
    let seed: [u8; 16] = seed_bytes.try_into().expect("16-byte seed");
    let bits = attribute(html, "data-pow-bits").parse().expect("bits");
    (id, seed, bits)
}

fn raw_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn login_page(server: &TestServer) -> String {
    server
        .client()
        .get("/accounts/login/", None)
        .await
        .text()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_solved_challenge_completes_a_web_login() {
    let server = TestServer::start_with_auth_config(|config| {
        config.captcha_mode = infra::config::CaptchaMode::Always;
        config.captcha_difficulty_bits = 12;
    })
    .await;
    create_test_user(server.db.as_ref(), "pow@example.com", "password123").await;

    let html = login_page(&server).await;
    let (id, seed, bits) = challenge(&html);
    let nonce = solve(&seed, bits);

    let resp = raw_client()
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[
            ("email", "pow@example.com"),
            ("password", "password123"),
            ("pow_id", &id),
            ("pow_nonce", &nonce),
        ])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302, "body: {}", resp.text().await.unwrap());
    assert!(
        resp.headers()
            .get_all("set-cookie")
            .iter()
            .any(|value| value.to_str().unwrap_or("").contains("seahub-session=")),
        "a successful sign-in must set the session cookie"
    );
}

#[tokio::test]
async fn a_missing_or_replayed_answer_is_refused() {
    let server = TestServer::start_with_auth_config(|config| {
        config.captcha_mode = infra::config::CaptchaMode::Always;
        config.captcha_difficulty_bits = 12;
    })
    .await;
    create_test_user(server.db.as_ref(), "pow@example.com", "password123").await;

    let html = login_page(&server).await;
    let (id, seed, bits) = challenge(&html);
    let nonce = solve(&seed, bits);

    // Missing fields: the form comes back with a fresh challenge.
    let resp = raw_client()
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", "pow@example.com"), ("password", "password123")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains(r#"name="pow_id""#),
        "a fresh challenge is rendered"
    );

    // A valid answer is still consumable only once.
    let first = raw_client()
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[
            ("email", "pow@example.com"),
            ("password", "password123"),
            ("pow_id", &id),
            ("pow_nonce", &nonce),
        ])
        .send()
        .await
        .unwrap();
    // The challenge was already consumed by the missing-fields attempt? No: that
    // request had no id, so it could not consume this one.
    assert_eq!(first.status(), 302);

    let replay = raw_client()
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[
            ("email", "pow@example.com"),
            ("password", "password123"),
            ("pow_id", &id),
            ("pow_nonce", &nonce),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert!(
        replay
            .text()
            .await
            .unwrap()
            .contains("Browser verification"),
        "the replay must be rejected"
    );
}

#[tokio::test]
async fn adaptive_challenges_only_after_a_failure() {
    let server = TestServer::start_with_auth_config(|config| {
        config.captcha_mode = infra::config::CaptchaMode::Adaptive;
        config.captcha_difficulty_bits = 12;
        config.captcha_trigger_after = 1;
    })
    .await;
    create_test_user(server.db.as_ref(), "adaptive@example.com", "password123").await;

    let first = login_page(&server).await;
    assert!(
        !first.contains(r#"name="pow_id""#),
        "a fresh address is not challenged"
    );

    let failed = raw_client()
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", "adaptive@example.com"), ("password", "wrong")])
        .send()
        .await
        .unwrap();
    assert_eq!(failed.status(), 200);
    let html = failed.text().await.unwrap();
    assert!(
        html.contains(r#"name="pow_id""#),
        "the failed sign-in must now ask for a challenge"
    );

    let (id, seed, bits) = challenge(&html);
    let nonce = solve(&seed, bits);
    let ok = raw_client()
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[
            ("email", "adaptive@example.com"),
            ("password", "password123"),
            ("pow_id", &id),
            ("pow_nonce", &nonce),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 302);
}

#[tokio::test]
async fn the_api_login_is_never_challenged() {
    let server = TestServer::start_with_auth_config(|config| {
        config.captcha_mode = infra::config::CaptchaMode::Always;
        config.captcha_difficulty_bits = 12;
    })
    .await;
    create_test_user(server.db.as_ref(), "api@example.com", "password123").await;

    let resp = server
        .client()
        .login("api@example.com", "password123")
        .await;
    assert_eq!(
        resp.status(),
        200,
        "API clients cannot solve a browser challenge"
    );
}

#[tokio::test]
async fn registration_and_reset_render_a_challenge() {
    let server = TestServer::start_with_pow_and_mail().await;

    let register = server
        .client()
        .get("/accounts/register/", None)
        .await
        .text()
        .await
        .unwrap();
    assert!(register.contains(r#"name="pow_id""#));
    assert!(register.contains("data-pow-seed"));

    let reset = server
        .client()
        .get("/accounts/password/reset/", None)
        .await
        .text()
        .await
        .unwrap();
    assert!(reset.contains(r#"name="pow_id""#));
    assert!(reset.contains("data-pow-seed"));

    // Posting without the hidden answer is refused before any account/mail work.
    let refused = server
        .client()
        .post_form(
            "/accounts/register/",
            None,
            &[
                ("email", "new@example.com"),
                ("password1", "password123"),
                ("password2", "password123"),
            ],
        )
        .await;
    assert_eq!(refused.status(), 200);
    assert!(
        refused
            .text()
            .await
            .unwrap()
            .contains("Browser verification")
    );

    let refused_reset = server
        .client()
        .post_form(
            "/accounts/password/reset/",
            None,
            &[("email", "new@example.com")],
        )
        .await;
    assert_eq!(refused_reset.status(), 200);
    assert!(
        refused_reset
            .text()
            .await
            .unwrap()
            .contains("Browser verification")
    );
}

#[tokio::test]
async fn off_keeps_the_forms_unchanged() {
    let server = TestServer::start().await;
    let login = login_page(&server).await;
    assert!(!login.contains(r#"name="pow_id""#));

    let register = server
        .client()
        .get("/accounts/register/", None)
        .await
        .text()
        .await
        .unwrap();
    assert!(!register.contains(r#"name="pow_id""#));
}
