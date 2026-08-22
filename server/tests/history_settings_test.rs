//! Tests for per-repo history retention settings (history_limit / history_ttl_days).

mod common;

use common::TestFixture;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use server::fs::core::GcManager;

/// A throwaway filesystem-backed block store for GC calls. These pruned-history
/// tests never write blocks, so an empty store suffices.
fn temp_block_store() -> (tempfile::TempDir, infra::storage::DynBlockStorage) {
    let dir = tempfile::tempdir().unwrap();
    let store = infra::storage::new_block_store(dir.path());
    (dir, store)
}

/// Insert a single fs object row directly into the DB.
async fn insert_fs_obj(
    db: &sea_orm::DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
    obj_type: i8,
    data: serde_json::Value,
) {
    let obj = infra::entity::fs_object::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_id.to_string()),
        fs_id: Set(fs_id.to_string()),
        obj_type: Set(obj_type),
        data: Set(data.to_string()),
    };
    obj.insert(db).await.unwrap();
}

/// Insert a commit row pointing at `root_id` (which must already exist).
async fn insert_commit(
    db: &sea_orm::DatabaseConnection,
    repo_id: &str,
    commit_id: &str,
    root_id: &str,
    parent_id: Option<String>,
    ctime: i64,
) {
    let commit = infra::entity::commit::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_id.to_string()),
        commit_id: Set(commit_id.to_string()),
        root_id: Set(root_id.to_string()),
        parent_id: Set(parent_id),
        second_parent_id: sea_orm::NotSet,
        creator_name: Set("test@example.com".to_string()),
        creator: Set(base::common::EMPTY_SHA1.to_string()),
        description: Set("test commit".to_string()),
        ctime: Set(ctime),
        version: Set(1),
    };
    commit.insert(db).await.unwrap();
}

/// Insert a commit plus an empty root directory fs object.
async fn insert_commit_and_root(
    db: &sea_orm::DatabaseConnection,
    repo_id: &str,
    commit_id: &str,
    root_id: &str,
    parent_id: Option<String>,
    ctime: i64,
) {
    insert_fs_obj(
        db,
        repo_id,
        root_id,
        3,
        serde_json::json!({"dirents": [], "type": 3, "version": 1}),
    )
    .await;
    insert_commit(db, repo_id, commit_id, root_id, parent_id, ctime).await;
}

/// Set a repo's history retention settings directly in the DB.
async fn set_history(db: &sea_orm::DatabaseConnection, repo_id: &str, limit: i32, ttl_days: i32) {
    let mut repo: infra::entity::repo::ActiveModel =
        infra::entity::repo::Entity::find_by_id(repo_id)
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .into();
    repo.history_limit = Set(limit);
    repo.history_ttl_days = Set(ttl_days);
    repo.update(db).await.unwrap();
}

/// POST /api2/repos/{id}/?op=update persists history retention settings.
#[tokio::test]
async fn test_update_repo_history_settings() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/?op=update", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({"history_limit": 3, "history_ttl_days": 30}),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let repo = infra::entity::repo::Entity::find_by_id(&f.repo_id)
        .one(f.server.db.as_ref())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repo.history_limit, 3);
    assert_eq!(repo.history_ttl_days, 30);

    // Saving only name/description must not clobber the retention settings.
    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/?op=update", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({"repo_name": "renamed"}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let repo = infra::entity::repo::Entity::find_by_id(&f.repo_id)
        .one(f.server.db.as_ref())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repo.history_limit, 3);
    assert_eq!(repo.history_ttl_days, 30);
}

/// Negative retention values are rejected with 400.
#[tokio::test]
async fn test_update_repo_history_settings_rejects_negative() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/?op=update", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({"history_limit": -1}),
        )
        .await;
    assert_eq!(resp.status(), 400);

    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/?op=update", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({"history_ttl_days": -5}),
        )
        .await;
    assert_eq!(resp.status(), 400);
}

/// GC keeps the newest `history_limit` commits and prunes the rest.
#[tokio::test]
async fn test_gc_prunes_by_history_limit() {
    let f = TestFixture::new().await;
    let (_dir, block_store) = temp_block_store();
    let db = &*f.server.db;
    let now = chrono::Utc::now().timestamp();

    let old = "1".repeat(40);
    let mid = "2".repeat(40);
    let new = "3".repeat(40);
    // Oldest first; `now` is the newest commit.
    insert_commit_and_root(db, &f.repo_id, &"c".repeat(40), &old, None, now - 200).await;
    insert_commit_and_root(
        db,
        &f.repo_id,
        &"b".repeat(40),
        &mid,
        Some("c".repeat(40)),
        now - 100,
    )
    .await;
    insert_commit_and_root(
        db,
        &f.repo_id,
        &"a".repeat(40),
        &new,
        Some("b".repeat(40)),
        now,
    )
    .await;

    set_history(db, &f.repo_id, 2, 0).await;

    let removed = GcManager::garbage_collect(&f.server.repos, &block_store)
        .await
        .unwrap();
    assert_eq!(
        removed, 1,
        "only the oldest root fs object should be removed"
    );

    let commits = f
        .server
        .repos
        .commit
        .find_by_repo_id_ordered_by_ctime_desc(&f.repo_id, None)
        .await
        .unwrap();
    assert_eq!(commits.len(), 2, "oldest commit row should be pruned");

    assert!(
        f.server
            .repos
            .fs_object
            .find_by_repo_and_fs_id(&f.repo_id, &old)
            .await
            .unwrap()
            .is_none(),
        "oldest root fs object should be gone"
    );
    assert!(
        f.server
            .repos
            .fs_object
            .find_by_repo_and_fs_id(&f.repo_id, &mid)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        f.server
            .repos
            .fs_object
            .find_by_repo_and_fs_id(&f.repo_id, &new)
            .await
            .unwrap()
            .is_some()
    );
}

/// GC prunes commits older than `history_ttl_days`.
#[tokio::test]
async fn test_gc_prunes_by_ttl() {
    let f = TestFixture::new().await;
    let (_dir, block_store) = temp_block_store();
    let db = &*f.server.db;
    let now = chrono::Utc::now().timestamp();

    let stale = "4".repeat(40);
    let fresh = "5".repeat(40);
    // One commit 10 days old, one just now.
    insert_commit_and_root(
        db,
        &f.repo_id,
        &"x".repeat(40),
        &stale,
        None,
        now - 10 * 86_400,
    )
    .await;
    insert_commit_and_root(
        db,
        &f.repo_id,
        &"y".repeat(40),
        &fresh,
        Some("x".repeat(40)),
        now,
    )
    .await;

    set_history(db, &f.repo_id, 0, 5).await;

    let removed = GcManager::garbage_collect(&f.server.repos, &block_store)
        .await
        .unwrap();
    assert_eq!(
        removed, 1,
        "only the stale root fs object should be removed"
    );

    let commits = f
        .server
        .repos
        .commit
        .find_by_repo_id_ordered_by_ctime_desc(&f.repo_id, None)
        .await
        .unwrap();
    assert_eq!(commits.len(), 1);
    assert!(
        f.server
            .repos
            .fs_object
            .find_by_repo_and_fs_id(&f.repo_id, &stale)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.server
            .repos
            .fs_object
            .find_by_repo_and_fs_id(&f.repo_id, &fresh)
            .await
            .unwrap()
            .is_some()
    );
}

/// Repos with both settings at 0 (unlimited) are left untouched by GC.
#[tokio::test]
async fn test_gc_noop_when_unlimited() {
    let f = TestFixture::new().await;
    let (_dir, block_store) = temp_block_store();
    let db = &*f.server.db;
    let now = chrono::Utc::now().timestamp();

    let r1 = "6".repeat(40);
    let r2 = "7".repeat(40);
    insert_commit_and_root(
        db,
        &f.repo_id,
        &"p".repeat(40),
        &r1,
        None,
        now - 10 * 86_400,
    )
    .await;
    insert_commit_and_root(
        db,
        &f.repo_id,
        &"q".repeat(40),
        &r2,
        Some("p".repeat(40)),
        now,
    )
    .await;

    // history_limit = 0, history_ttl_days = 0 → unlimited.
    set_history(db, &f.repo_id, 0, 0).await;

    let removed = GcManager::garbage_collect(&f.server.repos, &block_store)
        .await
        .unwrap();
    assert_eq!(removed, 0);

    let commits = f
        .server
        .repos
        .commit
        .find_by_repo_id_ordered_by_ctime_desc(&f.repo_id, None)
        .await
        .unwrap();
    assert_eq!(commits.len(), 2);
    assert!(
        f.server
            .repos
            .fs_object
            .find_by_repo_and_fs_id(&f.repo_id, &r1)
            .await
            .unwrap()
            .is_some()
    );
}

/// GC's reachability walk descends through nested directories (BFS batching).
#[tokio::test]
async fn test_gc_collects_nested_fs_ids() {
    let (_dir, block_store) = temp_block_store();
    use infra::serialization::S_IFDIR;
    use infra::serialization::S_IFREG;

    let f = TestFixture::new().await;
    let db = &*f.server.db;
    let now = chrono::Utc::now().timestamp();

    let r_old = "a".repeat(40);
    let f_old = "b".repeat(40);
    let r_new = "c".repeat(40);
    let d1 = "d".repeat(40);
    let g1 = "e".repeat(40);
    let file1 = "f".repeat(40);

    // Old commit: root → file f_old.
    insert_fs_obj(
        db,
        &f.repo_id,
        &f_old,
        1,
        serde_json::json!({"block_ids": [], "size": 1, "type": 1, "version": 1}),
    )
    .await;
    insert_fs_obj(
        db,
        &f.repo_id,
        &r_old,
        3,
        serde_json::json!({
            "dirents": [{"id": f_old, "mode": S_IFREG, "mtime": now, "name": "old.txt"}],
            "type": 3,
            "version": 1
        }),
    )
    .await;
    insert_commit(db, &f.repo_id, &"g".repeat(40), &r_old, None, now - 200).await;

    // New commit: root → subdir d1 + file file1; d1 → file g1.
    insert_fs_obj(
        db,
        &f.repo_id,
        &file1,
        1,
        serde_json::json!({"block_ids": [], "size": 1, "type": 1, "version": 1}),
    )
    .await;
    insert_fs_obj(
        db,
        &f.repo_id,
        &g1,
        1,
        serde_json::json!({"block_ids": [], "size": 1, "type": 1, "version": 1}),
    )
    .await;
    insert_fs_obj(
        db,
        &f.repo_id,
        &d1,
        3,
        serde_json::json!({
            "dirents": [{"id": g1, "mode": S_IFREG, "mtime": now, "name": "g.txt"}],
            "type": 3,
            "version": 1
        }),
    )
    .await;
    insert_fs_obj(
        db,
        &f.repo_id,
        &r_new,
        3,
        serde_json::json!({
            "dirents": [
                {"id": d1, "mode": S_IFDIR, "mtime": now, "name": "sub"},
                {"id": file1, "mode": S_IFREG, "mtime": now, "name": "file.txt"}
            ],
            "type": 3,
            "version": 1
        }),
    )
    .await;
    insert_commit(
        db,
        &f.repo_id,
        &"h".repeat(40),
        &r_new,
        Some("g".repeat(40)),
        now,
    )
    .await;

    set_history(db, &f.repo_id, 1, 0).await;

    let removed = GcManager::garbage_collect(&f.server.repos, &block_store)
        .await
        .unwrap();
    assert_eq!(removed, 2, "old root and old file should be removed");

    let commits = f
        .server
        .repos
        .commit
        .find_by_repo_id_ordered_by_ctime_desc(&f.repo_id, None)
        .await
        .unwrap();
    assert_eq!(commits.len(), 1);

    // All objects reachable from the kept commit survive, including nested ones.
    for kept in [&r_new, &d1, &g1, &file1] {
        assert!(
            f.server
                .repos
                .fs_object
                .find_by_repo_and_fs_id(&f.repo_id, kept)
                .await
                .unwrap()
                .is_some(),
            "fs_id {} should be retained",
            kept
        );
    }
    for gone in [&r_old, &f_old] {
        assert!(
            f.server
                .repos
                .fs_object
                .find_by_repo_and_fs_id(&f.repo_id, gone)
                .await
                .unwrap()
                .is_none(),
            "fs_id {} should be removed",
            gone
        );
    }
}
