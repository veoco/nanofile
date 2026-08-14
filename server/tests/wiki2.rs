//! Integration tests for the Seafile wiki2 (知识库) feature.
//!
//! A wiki is a library marked `type='wiki'`; pages are markdown files under
//! `/wiki-pages`, and the config lives in the hidden `_Internal/Wiki`.

mod common;

use common::TestFixture;

/// The client's 知识库 tab only appears when server-info advertises `wiki`.
#[tokio::test]
async fn test_server_info_features_include_wiki() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let features = body["features"].as_array().expect("features array");
    let features: Vec<&str> = features.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        features.contains(&"wiki"),
        "server-info features must include \"wiki\" for the 知识库 tab to show: {features:?}"
    );
}

/// Create → list → rename → publish → unpublish → delete, as the mobile
/// clients drive it.
#[tokio::test]
async fn test_wiki_crud_flow() {
    let f = TestFixture::new().await;

    // Create a wiki.
    let resp = f
        .client
        .post_json(
            "/api/v2.1/wikis2/",
            Some(&f.api_token),
            &serde_json::json!({"name": "engineering-wiki"}),
        )
        .await;
    assert_eq!(resp.status(), 201, "create wiki should return 201");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "engineering-wiki");
    assert_eq!(body["type"], "mine");
    assert_eq!(body["permission"], "rw");
    let wiki_id = body["id"]
        .as_str()
        .expect("wiki id is the repo_id")
        .to_string();

    // List wikis.
    let resp = f.client.get("/api/v2.1/wikis2/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let wikis = body["wikis"].as_array().expect("wikis array");
    assert!(!wikis.is_empty(), "created wiki should appear in the list");
    assert!(wikis.iter().any(|w| w["id"] == wiki_id));
    assert_eq!(body["group_wikis"], serde_json::json!([]));

    // Legacy wiki1 endpoint returns an empty data array.
    let resp = f.client.get("/api/v2.1/wikis/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"], serde_json::json!([]));

    // Rename.
    let resp = f
        .client
        .put_json(
            &format!("/api/v2.1/wiki2/{wiki_id}/"),
            Some(&f.api_token),
            &serde_json::json!({"wiki_name": "renamed-wiki"}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);

    // Publish.
    let resp = f
        .client
        .post_json(
            &format!("/api/v2.1/wiki2/{wiki_id}/publish/"),
            Some(&f.api_token),
            &serde_json::json!({"publish_url": "engwiki"}),
        )
        .await;
    if resp.status() != 200 {
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        eprintln!("PUBLISH RESPONSE: status={status} body={txt}");
        panic!("publish failed");
    }
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["publish_url"], "engwiki");

    // List again — the wiki should now be published.
    let resp = f.client.get("/api/v2.1/wikis2/", Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let wiki = body["wikis"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == wiki_id)
        .expect("wiki in list");
    assert_eq!(wiki["is_published"], true);
    assert_eq!(wiki["public_url_suffix"], "engwiki");

    // Publish info.
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/wiki2/{wiki_id}/publish/"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["publish_url"], "engwiki");

    // Unpublish.
    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/wiki2/{wiki_id}/publish/"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Invalid publish url is rejected (too short / bad chars).
    let resp = f
        .client
        .post_json(
            &format!("/api/v2.1/wiki2/{wiki_id}/publish/"),
            Some(&f.api_token),
            &serde_json::json!({"publish_url": "ab"}),
        )
        .await;
    assert_eq!(resp.status(), 400, "short publish url must be rejected");

    // Delete.
    let resp = f
        .client
        .delete(&format!("/api/v2.1/wiki2/{wiki_id}/"), Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f.client.get("/api/v2.1/wikis2/", Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        !body["wikis"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == wiki_id),
        "deleted wiki must not be listed"
    );
}

/// A wiki's internal storage (`_Internal`, `/wiki-pages`) must be hidden from
/// the file browser and downloads.
#[tokio::test]
async fn test_wiki_internal_storage_hidden() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/wikis2/",
            Some(&f.api_token),
            &serde_json::json!({"name": "hidden-wiki"}),
        )
        .await;
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let wiki_id = body["id"].as_str().unwrap().to_string();

    // The dir listing of the wiki repo must not expose the internal dirs.
    let resp = f.client.list_dir(&f.api_token, &wiki_id, "/").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(
        !names.contains(&"_Internal"),
        "_Internal must be hidden from the wiki repo listing: {names:?}"
    );
    assert!(
        !names.contains(&"wiki-pages"),
        "wiki-pages must be hidden from the wiki repo listing: {names:?}"
    );

    // Direct download of the internal config must 404.
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{wiki_id}/file/?p=/_Internal/Wiki/index.json&reuse=0"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        404,
        "_Internal config must not be downloadable"
    );
}

/// config / pages / page API: a new wiki has a home page; pages can be
/// created, listed, locked, renamed and deleted.
#[tokio::test]
async fn test_wiki_config_and_pages() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/wikis2/",
            Some(&f.api_token),
            &serde_json::json!({"name": "pages-wiki"}),
        )
        .await;
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let wiki_id = body["id"].as_str().unwrap().to_string();

    // Fresh wiki config has exactly one home page.
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/wiki2/{wiki_id}/config/"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let config = &body["wiki"]["wiki_config"];
    let pages = config["pages"].as_array().expect("pages array");
    assert_eq!(pages.len(), 1, "new wiki should have a home page");
    let home_id = pages[0]["id"].as_str().unwrap().to_string();

    // Create a second page under the home page.
    let resp = f
        .client
        .post_json(
            &format!("/api/v2.1/wiki2/{wiki_id}/pages/"),
            Some(&f.api_token),
            &serde_json::json!({
                "page_name": "getting-started",
                "current_id": home_id,
                "insert_position": "inner",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create page should succeed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let new_page_id = body["file_info"]["page_id"].as_str().unwrap().to_string();

    // Config now has two pages and a nested nav node.
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/wiki2/{wiki_id}/config/"),
            Some(&f.api_token),
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let config = &body["wiki"]["wiki_config"];
    assert_eq!(config["pages"].as_array().unwrap().len(), 2);
    let nav_home = config["navigation"].as_array().unwrap()[0].clone();
    let child_ids: Vec<&str> = nav_home["children"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect();
    assert!(
        child_ids.contains(&new_page_id.as_str()),
        "new page should be nested under home: {child_ids:?}"
    );

    // Get the new page metadata.
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/wiki2/{wiki_id}/page/{new_page_id}/"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "getting-started");

    // Rename the page via page config.
    let resp = f
        .client
        .put_json(
            &format!("/api/v2.1/wiki2/{wiki_id}/page/{new_page_id}/config/"),
            Some(&f.api_token),
            &serde_json::json!({"page_name": "renamed-page"}),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Lock the page.
    let resp = f
        .client
        .put_json(
            &format!("/api/v2.1/wiki2/{wiki_id}/page/{new_page_id}/"),
            Some(&f.api_token),
            &serde_json::json!({"is_lock_page": true}),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Delete the page.
    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/wiki2/{wiki_id}/page/{new_page_id}/"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Config is back to a single page.
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/wiki2/{wiki_id}/config/"),
            Some(&f.api_token),
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["wiki"]["wiki_config"]["pages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// The wiki web page (`/wikis/{repo_id}/`) renders the navigation tree and
/// rendered markdown, and the edit-save round trip persists content.
#[tokio::test]
async fn test_wiki_web_page_render_and_edit() {
    let f = TestFixture::new().await;

    // Create a wiki with some markdown content on the home page.
    let resp = f
        .client
        .post_json(
            "/api/v2.1/wikis2/",
            Some(&f.api_token),
            &serde_json::json!({"name": "web-wiki"}),
        )
        .await;
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let wiki_id = body["id"].as_str().unwrap().to_string();

    // Log into the web UI with a cookie jar.
    let cookie_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();
    let login = cookie_client
        .post(format!("{}/accounts/login/", f.server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 302, "web login should redirect");

    // View the wiki page — it should render the home page.
    let resp = cookie_client
        .get(format!("{}/wikis/{wiki_id}/", f.server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.unwrap();
    assert!(html.contains("web-wiki"), "wiki name should be in the page");
    assert!(
        html.contains("home"),
        "home page nav link should be present"
    );

    // Edit the home page.
    let config_resp = f
        .client
        .get(
            &format!("/api/v2.1/wiki2/{wiki_id}/config/"),
            Some(&f.api_token),
        )
        .await;
    let config_body: serde_json::Value = config_resp.json().await.unwrap();
    let home_id = config_body["wiki"]["wiki_config"]["pages"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let edit = cookie_client
        .get(format!(
            "{}/wikis/{wiki_id}/page/{home_id}/edit/",
            f.server.base_url
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(edit.status(), 200);
    let edit_html = edit.text().await.unwrap();
    let csrf = extract_form_csrf(&edit_html).expect("edit form csrf token");

    // Save new markdown.
    let save = cookie_client
        .post(format!(
            "{}/wikis/{wiki_id}/page/{home_id}/save/",
            f.server.base_url
        ))
        .form(&[
            ("csrf_token", csrf.as_str()),
            ("content", "# Hello\n\n**bold** text"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(save.status(), 302, "save should redirect back to the page");

    // Re-render — the page should contain the rendered markdown.
    let resp = cookie_client
        .get(format!(
            "{}/wikis/{wiki_id}/?page_id={home_id}",
            f.server.base_url
        ))
        .send()
        .await
        .unwrap();
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("<strong>bold</strong>"),
        "markdown should be rendered as HTML"
    );
}

/// Extract the CSRF token hidden input from an HTML form.
fn extract_form_csrf(html: &str) -> Option<String> {
    let marker = "name=\"csrf_token\" value=\"";
    let start = html.find(marker)? + marker.len();
    let end = html[start..].find('"')? + start;
    Some(html[start..end].to_string())
}
