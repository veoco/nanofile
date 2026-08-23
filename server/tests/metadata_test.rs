mod common;

use common::{TestFixture, create_test_user};

/// `GET /api/v2.1/repos/{repo_id}/related-users/` must return a `user_list` of
/// `{email, name, contact_email, avatar_url}` objects (seahub's
/// `get_user_common_info` shape), not the legacy `{"users": ["<user_id>"]}`.
/// The Android client's `UserWrapperModel.user_list` NPEs when the field is
/// missing or holds strings.
#[tokio::test]
async fn test_related_users_returns_user_list_objects() {
    let f = TestFixture::new().await;

    // Add a second user as a repo member.
    let user2_id = create_test_user(f.server.db.as_ref(), "second@example.com", "password").await;
    let now = chrono::Utc::now().timestamp();
    f.server
        .repos
        .member
        .create_member(server::repository::member::CreateMemberParams {
            repo_id: f.repo_id.clone(),
            user_id: user2_id,
            permission: "rw".to_string(),
            created_at: now,
        })
        .await
        .unwrap();

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/related-users/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let user_list = body["user_list"]
        .as_array()
        .expect("response should contain a user_list array");

    // Owner + the second member.
    assert_eq!(user_list.len(), 2, "expected 2 related users: {body}");

    let emails: Vec<&str> = user_list
        .iter()
        .map(|u| u["email"].as_str().unwrap())
        .collect();
    assert!(emails.contains(&"test@example.com"), "emails: {emails:?}");
    assert!(emails.contains(&"second@example.com"), "emails: {emails:?}");

    for u in user_list {
        assert!(u["email"].as_str().is_some(), "email missing: {u}");
        assert!(u["name"].as_str().is_some(), "name missing: {u}");
        assert!(
            u["contact_email"].as_str().is_some(),
            "contact_email missing: {u}"
        );
        assert!(
            u["avatar_url"].as_str().is_some(),
            "avatar_url missing: {u}"
        );
    }
}

/// `GET .../metadata/record/` must return a non-empty `metadata` column list.
/// The Android file-profile dialog/editor iterate `metadata` to decide which
/// fields to render; an empty list (the previous behavior) renders nothing.
#[tokio::test]
async fn test_metadata_record_returns_system_columns() {
    let f = TestFixture::new().await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "photo.jpg", b"jpeg-data")
        .await;

    let resp = f
        .client
        .get(
            &format!(
                "/api/v2.1/repos/{}/metadata/record/?parent_dir=/&name=photo.jpg&file_name=photo.jpg",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let columns = body["metadata"]
        .as_array()
        .expect("response should contain a metadata column array");

    let expected: &[(&str, &str)] = &[
        ("_size", "number"),
        ("_file_modifier", "text"),
        ("_file_mtime", "date"),
        ("_tags", "link"),
        ("_description", "long-text"),
    ];

    for (key, ty) in expected {
        let col = columns
            .iter()
            .find(|c| c["key"].as_str() == Some(*key))
            .unwrap_or_else(|| panic!("missing column {key}: {body}"));
        assert_eq!(col["name"].as_str(), Some(*key), "name mismatch for {key}");
        assert_eq!(col["type"].as_str(), Some(*ty), "type mismatch for {key}");
    }

    // The record itself must carry values for the rendered fields.
    let rec = &body["results"][0];
    assert!(rec["_size"].is_number(), "_size missing: {body}");
    assert!(rec["_file_modifier"].is_string(), "_file_modifier missing");
    assert!(rec["_file_mtime"].is_string(), "_file_mtime missing");
    assert!(rec["_tags"].is_array(), "_tags missing: {body}");
}

/// `GET .../metadata/tag-files/{tag_id}/` paginates via `start`/`limit` and
/// stays backwards compatible (returns everything) when the params are omitted.
#[tokio::test]
async fn test_tag_files_paginates() {
    let f = TestFixture::new().await;

    // Create a tag.
    let resp = f
        .client
        .post_json(
            &format!("/api/v2.1/repos/{}/metadata/tags/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "tags_data": [{ "_tag_name": "重要", "_tag_color": "#FF0000" }]
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create tag failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let tag_id = body["tags"][0]["_id"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .expect("tag id missing");

    // Upload 5 files and tag them all.
    let mut file_tags_data = Vec::new();
    for i in 0..5 {
        let name = format!("f{i}.txt");
        let up = f
            .client
            .upload_file(&f.api_token, &f.repo_id, "/", &name, b"x")
            .await;
        assert!(up.status().is_success(), "upload {name} failed");
        let record_id = server::service::fs::metadata::MetadataService::record_id_from_path(
            &format!("/{name}"),
        );
        file_tags_data.push(serde_json::json!({
            "record_id": record_id,
            "tags": [tag_id.to_string()],
        }));
    }
    let resp = f
        .client
        .put_json(
            &format!("/api/v2.1/repos/{}/metadata/file-tags/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({ "file_tags_data": file_tags_data }),
        )
        .await;
    assert_eq!(resp.status(), 200, "set tags failed");

    // Page 1 → 3 entries.
    let resp = f
        .client
        .get(
            &format!(
                "/api/v2.1/repos/{}/metadata/tag-files/{}/?start=0&limit=3",
                f.repo_id, tag_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["results"].as_array().unwrap().len(), 3, "page 1");

    // Page 2 → remaining 2.
    let resp = f
        .client
        .get(
            &format!(
                "/api/v2.1/repos/{}/metadata/tag-files/{}/?start=3&limit=3",
                f.repo_id, tag_id
            ),
            Some(&f.api_token),
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["results"].as_array().unwrap().len(), 2, "page 2");

    // No params → everything (backwards compatible with existing clients).
    let resp = f
        .client
        .get(
            &format!(
                "/api/v2.1/repos/{}/metadata/tag-files/{}/",
                f.repo_id, tag_id
            ),
            Some(&f.api_token),
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["results"].as_array().unwrap().len(), 5, "no params");
}
