//! `nanofile verify-dir-order`: the directory-object order check and its repair.
//!
//! `fs_id` is the SHA-1 of a directory object's serialized form, so the order of
//! `dirents` is part of the object's identity: official clients write descending
//! name order and read what is stored back verbatim, which means a directory
//! stored in any other order carries an id no client can reproduce. These tests
//! pin down that the scan finds such objects, that a repair appends one ordinary
//! commit (history untouched, tree unchanged, idempotent), and that structural
//! damage is reported without being "repaired".

mod common;

use base::common::{DirEntryData, FsDirData, S_IFDIR, S_IFREG, SEAF_METADATA_TYPE_DIR};
use common::{TestFixture, sync_commit};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use server::domain::fs::{compute_dir, compute_fs_id, dir_to_json, sort_dirents_desc};
use server::fs::core::dir_order::DirOrder;
use server::fs::core::read_fs_dir_data;

const REG: i32 = S_IFREG;

async fn head_commit_id(f: &TestFixture) -> Option<String> {
    infra::entity::repo::Entity::find_by_id(&f.repo_id)
        .one(f.server.db.as_ref())
        .await
        .unwrap()
        .expect("fixture repo")
        .head_commit_id
}

async fn root_of(f: &TestFixture, commit_id: &str) -> String {
    infra::entity::commit::Entity::find()
        .filter(infra::entity::commit::Column::RepoId.eq(&f.repo_id))
        .filter(infra::entity::commit::Column::CommitId.eq(commit_id))
        .one(f.server.db.as_ref())
        .await
        .unwrap()
        .expect("commit")
        .root_id
}

async fn object_bytes(f: &TestFixture, fs_id: &str) -> Option<String> {
    infra::entity::fs_object::Entity::find()
        .filter(infra::entity::fs_object::Column::RepoId.eq(&f.repo_id))
        .filter(infra::entity::fs_object::Column::FsId.eq(fs_id))
        .one(f.server.db.as_ref())
        .await
        .unwrap()
        .map(|row| row.data)
}

fn dirent(name: &str, id: &str, mode: i32, size: i64) -> DirEntryData {
    DirEntryData {
        id: id.to_string(),
        mode,
        modifier: "test@example.com".to_string(),
        mtime: 1_700_000_000,
        name: name.to_string(),
        size,
    }
}

/// Store `data` as a directory object and make it the library's HEAD, the way a
/// pre-fix server (or any REST write) would have stored whatever order it had.
/// Returns `(root_id, commit_id)`.
async fn plant_root(f: &TestFixture, data: &FsDirData) -> (String, String) {
    let json = dir_to_json(data);
    let root_id = compute_fs_id(&json);
    let db = f.server.db.as_ref();

    infra::entity::fs_object::Entity::insert(infra::entity::fs_object::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(f.repo_id.clone()),
        fs_id: Set(root_id.clone()),
        obj_type: Set(SEAF_METADATA_TYPE_DIR as i8),
        data: Set(json),
    })
    .exec(db)
    .await
    .unwrap();

    let parent = head_commit_id(f).await;
    let commit_id = common::fs_object_id(format!("dir-order:{}", root_id).as_bytes());
    infra::entity::commit::Entity::insert(infra::entity::commit::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(f.repo_id.clone()),
        commit_id: Set(commit_id.clone()),
        root_id: Set(root_id.clone()),
        parent_id: Set(parent),
        second_parent_id: Set(None),
        creator_name: Set("test@example.com".to_string()),
        creator: Set("0".repeat(40)),
        description: Set("planted root".to_string()),
        ctime: Set(chrono::Utc::now().timestamp()),
        version: Set(1i8),
        new_merge: Set(None),
        conflict: Set(None),
    })
    .exec(db)
    .await
    .unwrap();

    infra::entity::repo::Entity::update_many()
        .filter(infra::entity::repo::Column::Id.eq(&f.repo_id))
        .set(infra::entity::repo::ActiveModel {
            head_commit_id: Set(Some(commit_id.clone())),
            ..Default::default()
        })
        .exec(db)
        .await
        .unwrap();

    (root_id, commit_id)
}

/// A directory object written by a server-side store is stored in canonical
/// order even when the caller built it in insertion order, and its id is the
/// hash of exactly those bytes.
#[tokio::test]
async fn stored_dir_objects_are_canonical() {
    let f = TestFixture::new().await;

    let data = FsDirData {
        dirents: vec![
            dirent("a.txt", &"a".repeat(40), REG, 1),
            dirent("b", &"b".repeat(40), S_IFDIR, 0),
        ],
        obj_type: SEAF_METADATA_TYPE_DIR,
        version: 1,
    };
    let fs_id = server::fs::core::store_fs_dir_object(f.server.db.as_ref(), &f.repo_id, &data)
        .await
        .unwrap();

    let stored = object_bytes(&f, &fs_id).await.expect("stored object");
    let parsed: FsDirData = serde_json::from_str(&stored).unwrap();
    let names: Vec<&str> = parsed.dirents.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["b", "a.txt"],
        "descending, like compare_dirents()"
    );
    assert_eq!(
        compute_fs_id(&stored),
        fs_id,
        "the id must hash the stored bytes"
    );
    assert_eq!(
        compute_dir(&parsed).unwrap().0,
        fs_id,
        "re-serializing what was stored must reproduce the id"
    );
}

/// A head the pre-fix server could write (ascending entries) is reported, and
/// `repair` gives the library one ordinary commit with the canonical root.
#[tokio::test]
async fn a_non_canonical_head_is_reported_and_repaired() {
    let f = TestFixture::new().await;

    // `sync_commit` computes the object id from the entries in the order given,
    // and the sync path stores uploaded objects verbatim: ascending order right
    // here is exactly the defect this command repairs.
    let head = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[
            ("a.txt", b"a".as_slice(), REG),
            ("b.txt", b"b".as_slice(), REG),
        ],
        &[],
        None,
    )
    .await;
    let old_root = root_of(&f, &head).await;

    let report = DirOrder::run(f.server.db.as_ref(), Some(&f.repo_id), false)
        .await
        .unwrap();
    assert!(report.has_findings());
    assert_eq!(
        report.unordered.len(),
        1,
        "one non-canonical root: {report:?}"
    );
    let finding = &report.unordered[0];
    assert_eq!(finding.repo_id, f.repo_id);
    assert_eq!(finding.path, "/");
    assert_eq!(finding.stored_id, old_root);
    assert_ne!(finding.canonical_id, old_root);

    // entries.
    let mut sorted = read_fs_dir_data(f.server.repos.as_ref(), &f.repo_id, &old_root)
        .await
        .unwrap();
    sort_dirents_desc(&mut sorted.dirents);
    assert_eq!(compute_dir(&sorted).unwrap().0, finding.canonical_id);

    assert!(report.repairs.is_empty());
    assert_eq!(head_commit_id(&f).await.as_deref(), Some(head.as_str()));

    let report = DirOrder::run(f.server.db.as_ref(), Some(&f.repo_id), true)
        .await
        .unwrap();
    assert!(report.repair_errors.is_empty(), "{report:?}");
    assert_eq!(report.repairs.len(), 1, "{report:?}");
    let repair = &report.repairs[0];
    assert_eq!(repair.head_before, head);
    assert_ne!(repair.head_after, head);
    assert_eq!(repair.dirs_rewritten, 1);

    assert_eq!(
        head_commit_id(&f).await.as_deref(),
        Some(repair.head_after.as_str())
    );
    assert_eq!(root_of(&f, &repair.head_after).await, finding.canonical_id);
    let commit = infra::entity::commit::Entity::find()
        .filter(infra::entity::commit::Column::RepoId.eq(&f.repo_id))
        .filter(infra::entity::commit::Column::CommitId.eq(&repair.head_after))
        .one(f.server.db.as_ref())
        .await
        .unwrap()
        .expect("repair commit");
    assert_eq!(commit.parent_id.as_deref(), Some(head.as_str()));
    assert_eq!(commit.description, "Normalize directory entry order");

    assert!(object_bytes(&f, &old_root).await.is_some());

    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let mut names: Vec<&str> = body
        .as_array()
        .expect("dirent array")
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, vec!["a.txt", "b.txt"]);

    let report = DirOrder::run(f.server.db.as_ref(), Some(&f.repo_id), false)
        .await
        .unwrap();
    assert!(!report.has_findings(), "{report:?}");
    assert_eq!(
        head_commit_id(&f).await.as_deref(),
        Some(repair.head_after.as_str())
    );
}

/// Duplicate names and a dangling dirent are reported, but never "fixed": the
/// repair only re-orders, and this head is already in canonical order.
#[tokio::test]
async fn structural_findings_are_reported_without_repairing() {
    let f = TestFixture::new().await;

    let data = FsDirData {
        dirents: vec![
            dirent("gone", &"f".repeat(40), S_IFDIR, 0),
            dirent("dup", &"a".repeat(40), REG, 1),
            dirent("dup", &"b".repeat(40), REG, 2),
        ],
        obj_type: SEAF_METADATA_TYPE_DIR,
        version: 1,
    };
    let (root_id, _) = plant_root(&f, &data).await;

    let report = DirOrder::run(f.server.db.as_ref(), Some(&f.repo_id), true)
        .await
        .unwrap();
    assert!(report.unordered.is_empty(), "{report:?}");
    let kinds: Vec<&str> = report.other.iter().map(|f| f.kind.as_str()).collect();
    assert!(kinds.contains(&"duplicate-name"), "{report:?}");
    assert!(kinds.contains(&"dangling-dirent"), "{report:?}");
    assert!(report.has_findings());

    assert!(report.repairs.is_empty(), "{report:?}");
    assert_eq!(
        root_of(&f, &head_commit_id(&f).await.unwrap()).await,
        root_id
    );
    let after = read_fs_dir_data(f.server.repos.as_ref(), &f.repo_id, &root_id)
        .await
        .unwrap();
    assert_eq!(after.dirents.len(), 3);
}
