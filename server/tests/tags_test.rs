//! Integration tests for the file-tag feature (metadata-service API).

mod common;

use common::{TestFixture, create_test_user};

fn metadata_base(repo_id: &str) -> String {
    format!("/api/v2.1/repos/{repo_id}/metadata/")
}

fn tags_url(repo_id: &str) -> String {
    format!("{}tags/", metadata_base(repo_id))
}

fn file_tags_url(repo_id: &str) -> String {
    format!("{}file-tags/", metadata_base(repo_id))
}

fn record_url(repo_id: &str, parent_dir: &str, file_name: &str) -> String {
    format!(
        "{}record/?parent_dir={}&name={}&file_name={}",
        metadata_base(repo_id),
        parent_dir,
        file_name,
        file_name
    )
}

/// Create a tag and return its `_id` string.
async fn create_tag(f: &TestFixture, name: &str, color: &str) -> String {
    let resp = f
        .client
        .post_json(
            &tags_url(&f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({ "tags_data": [{ "_tag_name": name, "_tag_color": color }] }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    body["tags"][0]["_id"].as_str().unwrap().to_string()
}

/// GET /metadata/ exposes `tags_enabled` (mobile clients read this flag).
#[tokio::test]
async fn test_metadata_config_includes_tags_enabled() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(&metadata_base(&f.repo_id), Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["enabled"], true,
        "metadata enabled should default true"
    );
    assert_eq!(
        body["tags_enabled"], true,
        "tags_enabled should default true"
    );
}

#[tokio::test]
async fn test_tag_crud() {
    let f = TestFixture::new().await;
    let url = tags_url(&f.repo_id);

    // Create
    let resp = f
        .client
        .post_json(
            &url,
            Some(&f.api_token),
            &serde_json::json!({ "tags_data": [{ "_tag_name": "important", "_tag_color": "#ff0000" }] }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tag_id = body["tags"][0]["_id"].as_str().unwrap().to_string();
    assert_eq!(body["tags"][0]["_tag_name"], "important");
    assert_eq!(body["tags"][0]["_tag_color"], "#ff0000");

    // Duplicate create is deduped by name.
    let resp = f
        .client
        .post_json(
            &url,
            Some(&f.api_token),
            &serde_json::json!({ "tags_data": [{ "_tag_name": "important", "_tag_color": "#0000ff" }] }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["tags"].as_array().unwrap().len(),
        1,
        "duplicate name must not create a second tag"
    );
    assert_eq!(body["tags"][0]["_id"], tag_id);

    // List — metadata-service `{results, metadata}` shape.
    let resp = f.client.get(&url, Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .any(|t| t["_id"] == tag_id && t["_tag_name"] == "important"),
        "tag should appear in the repo tag list"
    );

    // Update (rename + recolor).
    let resp = f
        .client
        .put_json(
            &url,
            Some(&f.api_token),
            &serde_json::json!({ "tags_data": [{ "tag_id": tag_id, "tag": { "_tag_name": "urgent", "_tag_color": "#00ff00" } }] }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);

    let resp = f.client.get(&url, Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["_id"] == tag_id
                && t["_tag_name"] == "urgent"
                && t["_tag_color"] == "#00ff00")
    );

    // Delete (DELETE with JSON body).
    let del_client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = del_client
        .delete(format!("{}{}", f.server.base_url, url))
        .bearer_auth(&f.api_token)
        .json(&serde_json::json!({ "tag_ids": [tag_id] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = f.client.get(&url, Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["results"].as_array().unwrap().len(),
        0,
        "tag should be gone after delete"
    );
}

/// The mobile flow: record → set tags → verify → clear.
#[tokio::test]
async fn test_file_tags_flow() {
    let f = TestFixture::new().await;

    // Upload a file.
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "photo.jpg", b"jpeg-data")
        .await;
    assert_eq!(resp.status(), 200);

    let tag_id = create_tag(&f, "人物", "#ff9800").await;

    // GET record/ returns a record_id derived from the path.
    let rec_url = record_url(&f.repo_id, "/", "photo.jpg");
    let resp = f.client.get(&rec_url, Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let rec = &body["results"][0];
    let record_id = rec["_id"].as_str().unwrap().to_string();
    assert_eq!(rec["_name"], "photo.jpg");
    assert_eq!(rec["_parent_dir"], "/");
    assert_eq!(rec["_tags"].as_array().unwrap().len(), 0);

    // Set tags via file-tags/.
    let ft_url = file_tags_url(&f.repo_id);
    let resp = f
        .client
        .put_json(
            &ft_url,
            Some(&f.api_token),
            &serde_json::json!({ "file_tags_data": [{ "record_id": record_id, "tags": [tag_id] }] }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f.client.get(&rec_url, Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let tags = body["results"][0]["_tags"].as_array().unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(
        tags[0]["row_id"], tag_id,
        "record _tags must link the tag id"
    );
    assert_eq!(tags[0]["display_value"], "人物");

    // Clear by sending an empty array.
    let resp = f
        .client
        .put_json(
            &ft_url,
            Some(&f.api_token),
            &serde_json::json!({ "file_tags_data": [{ "record_id": record_id, "tags": [] }] }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f.client.get(&rec_url, Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["results"][0]["_tags"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_file_tags_invalid_record_id() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .put_json(
            &file_tags_url(&f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({ "file_tags_data": [{ "record_id": "zzz-not-hex", "tags": [] }] }),
        )
        .await;
    assert_eq!(resp.status(), 400, "invalid record_id must be rejected");
}

#[tokio::test]
async fn test_file_tags_unknown_tag_rejected() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .put_json(
            &file_tags_url(&f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({ "file_tags_data": [{ "record_id": "2f612e6a7067", "tags": ["99999"] }] }),
        )
        .await;
    assert_eq!(
        resp.status(),
        400,
        "a tag id not in the repo must be rejected"
    );
}

#[tokio::test]
async fn test_tags_require_membership() {
    let f = TestFixture::new().await;
    create_test_user(&f.server.db, "other@example.com", "password123").await;
    let resp = f.client.login("other@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let b_token = body["token"].as_str().unwrap().to_string();

    let resp = f.client.get(&tags_url(&f.repo_id), Some(&b_token)).await;
    assert_eq!(resp.status(), 403, "listing tags must require membership");

    let resp = f
        .client
        .put_json(
            &file_tags_url(&f.repo_id),
            Some(&b_token),
            &serde_json::json!({ "file_tags_data": [] }),
        )
        .await;
    assert_eq!(resp.status(), 403, "setting tags must require write access");
}

#[tokio::test]
async fn test_tagged_files() {
    let f = TestFixture::new().await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "a.pdf", b"pdf-a")
        .await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "b.pdf", b"pdf-b")
        .await;

    let tag_id = create_tag(&f, "文档", "#2196f3").await;

    // Tag only a.pdf.
    let rec_url = record_url(&f.repo_id, "/", "a.pdf");
    let resp = f.client.get(&rec_url, Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let record_id = body["results"][0]["_id"].as_str().unwrap().to_string();
    f.client
        .put_json(
            &file_tags_url(&f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({ "file_tags_data": [{ "record_id": record_id, "tags": [tag_id] }] }),
        )
        .await;

    // tag-files/ lists only the tagged file.
    let resp = f
        .client
        .get(
            &format!("{}tag-files/{}/", metadata_base(&f.repo_id), tag_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "only one file should carry the tag");
    assert_eq!(results[0]["_name"], "a.pdf");
}

#[tokio::test]
async fn test_tags_status_toggle() {
    let f = TestFixture::new().await;
    let url = format!("{}tags-status/", metadata_base(&f.repo_id));

    let resp = f.client.get(&url, Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["enabled"], true);

    let resp = f.client.delete(&url, Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let resp = f.client.get(&url, Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["enabled"], false,
        "tags should be disabled after DELETE"
    );

    let resp = f
        .client
        .put_json(
            &url,
            Some(&f.api_token),
            &serde_json::json!({ "lang": "en" }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let resp = f.client.get(&url, Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["enabled"], true);
}

/// Tags follow rename and are cleaned up on delete.
#[tokio::test]
async fn test_tag_path_sync_on_rename_and_delete() {
    let f = TestFixture::new().await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "old.txt", b"data")
        .await;

    let tag_id = create_tag(&f, "sync", "#ff5722").await;
    let rec_url = record_url(&f.repo_id, "/", "old.txt");
    let resp = f.client.get(&rec_url, Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let record_id = body["results"][0]["_id"].as_str().unwrap().to_string();
    f.client
        .put_json(
            &file_tags_url(&f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({ "file_tags_data": [{ "record_id": record_id, "tags": [tag_id] }] }),
        )
        .await;

    // Rename /old.txt → /new.txt.
    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/file/?p=/old.txt", f.repo_id),
            Some(&f.api_token),
            &[("operation", "rename"), ("newname", "new.txt")],
        )
        .await;
    assert_eq!(resp.status(), 200);

    // New path keeps the tag; old path no longer resolves.
    let new_rec = record_url(&f.repo_id, "/", "new.txt");
    let resp = f.client.get(&new_rec, Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["results"][0]["_tags"].as_array().unwrap().len(),
        1,
        "tag must follow the file to its new path"
    );
    let old_rec = record_url(&f.repo_id, "/", "old.txt");
    let resp = f.client.get(&old_rec, Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["results"][0]["_tags"].as_array().unwrap().len(),
        0,
        "old path must no longer carry the tag"
    );

    // Delete /new.txt → tag rows cleaned up.
    let del_client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = del_client
        .delete(format!(
            "{}/api2/repos/{}/file/?p=/new.txt",
            f.server.base_url, f.repo_id
        ))
        .bearer_auth(&f.api_token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // tag-files/ should now be empty.
    let resp = f
        .client
        .get(
            &format!("{}tag-files/{}/", metadata_base(&f.repo_id), tag_id),
            Some(&f.api_token),
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["results"].as_array().unwrap().len(),
        0,
        "deleted file's tag rows must be cleaned up"
    );
}

/// The web browser's folder-level tag filter (sort-bar) shows only tagged files.
#[tokio::test]
async fn test_folder_tag_filter_in_browser() {
    let f = TestFixture::new().await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "a.jpg", b"a-img")
        .await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "b.jpg", b"b-img")
        .await;

    let tag_id = create_tag(&f, "人物", "#ff9800").await;
    let rec_url = record_url(&f.repo_id, "/", "a.jpg");
    let resp = f.client.get(&rec_url, Some(&f.api_token)).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let record_id = body["results"][0]["_id"].as_str().unwrap().to_string();
    f.client
        .put_json(
            &file_tags_url(&f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({ "file_tags_data": [{ "record_id": record_id, "tags": [tag_id] }] }),
        )
        .await;

    // Log in through the web UI.
    let ui = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap();
    let resp = ui
        .post(format!("{}/accounts/login/", f.server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "web login should redirect on success");

    // Full listing contains both files.
    let list_url = reqwest::Url::parse_with_params(
        &format!("{}/libraries/{}/files", f.server.base_url, f.repo_id),
        &[("partial", "1"), ("view", "all")],
    )
    .unwrap();
    let html = ui.get(list_url).send().await.unwrap().text().await.unwrap();
    assert!(html.contains("a.jpg"), "full listing must include a.jpg");
    assert!(html.contains("b.jpg"), "full listing must include b.jpg");

    // Tag-filtered listing shows only a.jpg.
    let filter_url = reqwest::Url::parse_with_params(
        &format!("{}/libraries/{}/files", f.server.base_url, f.repo_id),
        &[("partial", "1"), ("view", "all"), ("tag", "人物")],
    )
    .unwrap();
    let html = ui
        .get(filter_url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        html.contains("a.jpg"),
        "tag filter must keep the tagged file"
    );
    assert!(
        !html.contains("b.jpg"),
        "tag filter must hide the untagged file"
    );
}
