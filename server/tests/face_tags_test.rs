//! End-to-end people tags: upload a portrait, let the scheduled scan run, and
//! verify the tags, the scan row and the correction path.

use std::time::Duration;

use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};

mod common;

async fn wait_for<F, Fut>(timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if predicate().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn uploading_a_portrait_creates_people_tags_and_a_correction() {
    let f = common::TestFixture::new_with_face_tags().await;
    let bytes = include_bytes!("fixtures/face_tags/portrait.jpg");
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "portrait.jpg", bytes)
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    let db = &*f.server.db;
    let finished = wait_for(Duration::from_secs(30), || async {
        let scan = infra::entity::face_tag_scan::Entity::find()
            .filter(infra::entity::face_tag_scan::Column::RepoId.eq(&f.repo_id))
            .filter(infra::entity::face_tag_scan::Column::FilePath.eq("/portrait.jpg"))
            .one(db)
            .await
            .ok()
            .flatten();
        scan.is_some_and(|row| row.status == "indexed" && row.face_count >= 1)
    })
    .await;
    if !finished {
        let scans = infra::entity::face_tag_scan::Entity::find()
            .all(db)
            .await
            .expect("scans");
        eprintln!("face scans: {scans:?}");
        let runs = infra::entity::job_run::Entity::find()
            .all(db)
            .await
            .expect("runs");
        eprintln!("job runs: {runs:?}");
    }
    assert!(finished, "the face scan should finish");

    let tags = infra::entity::repo_tag::Entity::find()
        .filter(infra::entity::repo_tag::Column::RepoId.eq(&f.repo_id))
        .all(db)
        .await
        .expect("tags");
    let any = tags
        .iter()
        .find(|tag| tag.label_key.as_deref() == Some("person_any"))
        .expect("person_any tag");
    assert_eq!(any.source, "vision");
    let single = tags
        .iter()
        .find(|tag| tag.label_key.as_deref() == Some("person_single"))
        .expect("person_single tag");

    let links = infra::entity::file_tag::Entity::find()
        .filter(infra::entity::file_tag::Column::RepoId.eq(&f.repo_id))
        .filter(infra::entity::file_tag::Column::FilePath.eq("/portrait.jpg"))
        .all(db)
        .await
        .expect("file tags");
    let ids: Vec<i32> = links.iter().map(|link| link.repo_tag_id).collect();
    assert!(ids.contains(&any.id));
    assert!(ids.contains(&single.id));

    // Remove only the broad "People" tag through the normal editor, leaving the
    // narrow one: the scan must remember that correction.
    let record_id = record_id_for("/portrait.jpg");
    let body = serde_json::json!({
        "file_tags_data": [{
            "record_id": record_id,
            "tags": [single.id.to_string()],
        }]
    });
    let resp = f
        .client
        .put_json(
            &format!("/api/v2.1/repos/{}/metadata/file-tags/", f.repo_id),
            Some(&f.api_token),
            &body,
        )
        .await;
    assert_eq!(resp.status(), 200, "tag edit should succeed");
    let exclusions = infra::entity::face_tag_exclusion::Entity::find()
        .filter(infra::entity::face_tag_exclusion::Column::RepoId.eq(&f.repo_id))
        .filter(infra::entity::face_tag_exclusion::Column::FilePath.eq("/portrait.jpg"))
        .all(db)
        .await
        .expect("exclusions");
    assert_eq!(exclusions.len(), 1);
    assert_eq!(exclusions[0].label_key, "person_any");
    assert_eq!(
        infra::entity::file_tag::Entity::find()
            .filter(infra::entity::file_tag::Column::RepoId.eq(&f.repo_id))
            .filter(infra::entity::file_tag::Column::FilePath.eq("/portrait.jpg"))
            .count(db)
            .await
            .expect("count"),
        1
    );

    // Adding it back is a new correction: the exclusion must go away so a
    // future scan can keep the tag.
    let body = serde_json::json!({
        "file_tags_data": [{
            "record_id": record_id_for("/portrait.jpg"),
            "tags": [any.id.to_string(), single.id.to_string()],
        }]
    });
    let resp = f
        .client
        .put_json(
            &format!("/api/v2.1/repos/{}/metadata/file-tags/", f.repo_id),
            Some(&f.api_token),
            &body,
        )
        .await;
    assert_eq!(resp.status(), 200);
    assert!(
        infra::entity::face_tag_exclusion::Entity::find()
            .filter(infra::entity::face_tag_exclusion::Column::RepoId.eq(&f.repo_id))
            .filter(infra::entity::face_tag_exclusion::Column::FilePath.eq("/portrait.jpg"))
            .all(db)
            .await
            .expect("exclusions")
            .is_empty()
    );

    // Tag names and their aliases are searchable without a full-text index.
    for query in ["people", "%E4%BA%BA%E7%89%A9"] {
        let resp = f
            .client
            .get(
                &format!("/api2/search/?q={query}&search_filename_only=false"),
                Some(&f.api_token),
            )
            .await;
        assert_eq!(resp.status(), 200, "search {query}");
        let body: serde_json::Value = resp.json().await.expect("search JSON");
        let paths: Vec<&str> = body["results"]
            .as_array()
            .expect("results")
            .iter()
            .filter_map(|hit| hit["fullpath"].as_str())
            .collect();
        assert!(
            paths.contains(&"/portrait.jpg"),
            "query {query} found {paths:?}"
        );
    }
}

fn record_id_for(path: &str) -> String {
    path.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
