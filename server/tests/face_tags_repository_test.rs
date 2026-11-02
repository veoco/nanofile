//! The path bookkeeping is raw SQL because SQLite cannot express a prefix
//! rewrite through SeaORM's update builder; these tests pin the exact/descendant
//! boundary and make sure a sibling with a shared prefix is untouched.

use std::sync::Arc;

use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait};
use sea_orm_migration::MigratorTrait;
use server::repository::face_tags::{DbFaceTagRepository, FaceTagRepository, ScanRecord};

async fn database() -> DatabaseConnection {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = migration::tests_support::connect(&migration::tests_support::file_uri(
        dir.path(),
        "test.db",
    ))
    .await;
    migration::Migrator::up(&db, None).await.expect("migrate");
    db.execute_unprepared("PRAGMA foreign_keys = OFF")
        .await
        .expect("disable foreign keys");
    // Keep the tempdir alive for the connection's lifetime by leaking it in a
    // test process; the OS cleans up at exit.
    std::mem::forget(dir);
    db
}

fn scan(path: &str) -> ScanRecord {
    ScanRecord {
        file_path: path.to_string(),
        fs_id: "f".repeat(40),
        model_version: 1,
        status: "indexed".to_string(),
        reason: None,
        face_count: 1,
        max_score: Some(0.9),
        attempted_at: 1,
    }
}

#[tokio::test]
async fn rename_moves_the_exact_path_and_descendants_not_a_shared_prefix() {
    let db = database().await;
    let repo = DbFaceTagRepository::new(Arc::new(db.clone()));
    for path in ["/a", "/a/b", "/a/b/c", "/ab", "/a2"] {
        repo.upsert_scan("repo-1", &scan(path)).await.expect("scan");
    }
    repo.exclude("repo-1", "/a/b", "person_any")
        .await
        .expect("exclude");

    repo.update_paths_for_rename("/a", "/z", "repo-1")
        .await
        .expect("rename");

    let mut paths: Vec<String> = infra::entity::face_tag_scan::Entity::find()
        .all(&db)
        .await
        .expect("paths")
        .into_iter()
        .map(|row| row.file_path)
        .collect();
    paths.sort();
    assert_eq!(paths, vec!["/a2", "/ab", "/z", "/z/b", "/z/b/c"]);
    assert_eq!(
        repo.list_exclusions("repo-1", "/z/b")
            .await
            .expect("exclusion"),
        vec!["person_any".to_string()]
    );
    let _ = db.close().await;
}

#[tokio::test]
async fn delete_removes_the_exact_path_and_descendants_not_a_sibling() {
    let db = database().await;
    let repo = DbFaceTagRepository::new(Arc::new(db.clone()));
    for path in ["/a", "/a/b", "/ab"] {
        repo.upsert_scan("repo-1", &scan(path)).await.expect("scan");
    }
    repo.delete_by_path_prefix("repo-1", "/a")
        .await
        .expect("delete");
    let paths: Vec<String> = infra::entity::face_tag_scan::Entity::find()
        .all(&db)
        .await
        .expect("paths")
        .into_iter()
        .map(|row| row.file_path)
        .collect();
    assert_eq!(paths, vec!["/ab"]);
    let _ = db.close().await;
}

#[tokio::test]
async fn exclusions_are_idempotent_and_clearable() {
    let db = database().await;
    let repo = DbFaceTagRepository::new(Arc::new(db.clone()));
    repo.exclude("repo-1", "/a", "person_any")
        .await
        .expect("exclude");
    repo.exclude("repo-1", "/a", "person_any")
        .await
        .expect("again");
    assert_eq!(
        repo.list_exclusions("repo-1", "/a").await.expect("list"),
        vec!["person_any".to_string()]
    );
    repo.include("repo-1", "/a", "person_any")
        .await
        .expect("include");
    assert!(
        repo.list_exclusions("repo-1", "/a")
            .await
            .expect("list")
            .is_empty()
    );
    let _ = db.close().await;
}

#[tokio::test]
async fn scan_rows_are_upserted_by_repo_and_path() {
    let db = database().await;
    let repo = DbFaceTagRepository::new(Arc::new(db.clone()));
    repo.upsert_scan("repo-1", &scan("/a"))
        .await
        .expect("insert");
    let mut updated = scan("/a");
    updated.face_count = 0;
    updated.status = "skipped".to_string();
    repo.upsert_scan("repo-1", &updated).await.expect("update");
    let row = repo
        .find_scan("repo-1", "/a")
        .await
        .expect("find")
        .expect("row");
    assert_eq!(row.face_count, 0);
    assert_eq!(row.status, "skipped");
    let count = infra::entity::face_tag_scan::Entity::find()
        .count(&db)
        .await
        .expect("count");
    assert_eq!(count, 1, "upsert must not insert a second row");
    let _ = db.close().await;
}
