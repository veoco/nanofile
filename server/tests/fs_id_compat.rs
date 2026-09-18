//! FS object identity must match seafile byte for byte: a `seafile`/`seafdir`
//! object id is the SHA-1 of `json_dumps(obj, JSON_SORT_KEYS)`.
//!
//! The golden strings below were produced by the **unmodified** upstream code
//! path (jansson 2.14 `json_dumps(object, JSON_SORT_KEYS)` over the object built
//! by `create_seafile_json()` / `seaf_dir_to_json()` + `add_to_dirent_array()`)
//! followed by SHA-1. Key order is alphabetical, members are separated by
//! `", "`, keys from values by `": "`, and `modifier`/`size` appear only on
//! regular-file dirents (`S_ISREG`).

use base::common::{
    DirEntryData, FsDirData, FsFileData, S_IFDIR, S_IFREG, SEAF_METADATA_TYPE_DIR,
    SEAF_METADATA_TYPE_FILE,
};

const FILE_JSON: &str = r#"{"block_ids": ["abcdef0123456789abcdef0123456789abcdef01", "1234567890abcdef1234567890abcdef12345678"], "size": 2048, "type": 1, "version": 1}"#;
const FILE_SHA1: &str = "c1656f97723c903b5736a62b626734027af1d782";

const DIR_JSON: &str = r#"{"dirents": [{"id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "mode": 33188, "modifier": "alice@example.com", "mtime": 1700000000, "name": "a.txt", "size": 123}, {"id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "mode": 16384, "mtime": 1700000001, "name": "sub"}], "type": 3, "version": 1}"#;
const DIR_SHA1: &str = "927d2119d636f99ed1a54ace24d23f9d399963ed";

fn file_fixture() -> FsFileData {
    FsFileData {
        block_ids: vec![
            "abcdef0123456789abcdef0123456789abcdef01".into(),
            "1234567890abcdef1234567890abcdef12345678".into(),
        ],
        size: 2048,
        obj_type: SEAF_METADATA_TYPE_FILE,
        version: 1,
    }
}

fn dir_fixture() -> FsDirData {
    FsDirData {
        dirents: vec![
            // Regular file: keeps modifier and size.
            DirEntryData {
                id: "a".repeat(40),
                mode: S_IFREG,
                modifier: "alice@example.com".into(),
                mtime: 1_700_000_000,
                name: "a.txt".into(),
                size: 123,
            },
            // Directory: modifier/size are absent from the JSON even though the
            // in-memory struct carries defaults for them.
            DirEntryData {
                id: "b".repeat(40),
                mode: S_IFDIR,
                modifier: String::new(),
                mtime: 1_700_000_001,
                name: "sub".into(),
                size: 0,
            },
        ],
        obj_type: SEAF_METADATA_TYPE_DIR,
        version: 1,
    }
}

#[test]
fn file_object_json_and_id_match_seafile() {
    let (fs_id, json) = server::domain::fs::compute_file(&file_fixture()).unwrap();
    assert_eq!(json, FILE_JSON, "file object JSON must match jansson");
    assert_eq!(fs_id, FILE_SHA1, "file object id must be sha1(json)");
}

#[test]
fn dir_object_json_and_id_match_seafile() {
    let (fs_id, json) = server::domain::fs::compute_dir(&dir_fixture()).unwrap();
    assert_eq!(json, DIR_JSON, "dir object JSON must match jansson");
    assert_eq!(fs_id, DIR_SHA1, "dir object id must be sha1(json)");
}

#[test]
fn compute_fs_id_is_sha1_of_json() {
    assert_eq!(server::domain::fs::compute_fs_id(FILE_JSON), FILE_SHA1);
    assert_eq!(server::domain::fs::compute_fs_id(DIR_JSON), DIR_SHA1);
}

/// A zero-byte file and an empty directory have no object at all: their id is
/// the `EMPTY_SHA1` sentinel (`seaf_fs_manager_index_blocks()` /
/// `seaf_dir_save()`), so serialization must refuse to produce one.
#[test]
fn empty_file_and_dir_have_no_object() {
    let empty_file = FsFileData {
        block_ids: vec![],
        size: 0,
        obj_type: SEAF_METADATA_TYPE_FILE,
        version: 1,
    };
    assert!(server::domain::fs::compute_file(&empty_file).is_none());

    let empty_dir = FsDirData {
        dirents: vec![],
        obj_type: SEAF_METADATA_TYPE_DIR,
        version: 1,
    };
    assert!(server::domain::fs::compute_dir(&empty_dir).is_none());
}

/// The serialization is deterministic (same input → same bytes → same id).
#[test]
fn fs_object_json_is_deterministic() {
    for _ in 0..3 {
        assert_eq!(server::domain::fs::file_to_json(&file_fixture()), FILE_JSON);
        assert_eq!(server::domain::fs::dir_to_json(&dir_fixture()), DIR_JSON);
    }
}

/// Regular files keep `modifier`/`size`; other entry types do not.
#[test]
fn dir_dirent_file_only_fields_are_conditional() {
    let json = server::domain::fs::dir_to_json(&dir_fixture());
    let file_part = json.split("}, {").next().unwrap();
    assert!(file_part.contains(r#""modifier": "alice@example.com""#));
    assert!(file_part.contains(r#""size": 123"#));

    let dir_part = json.split("}, {").nth(1).unwrap();
    assert!(
        !dir_part.contains("\"modifier\""),
        "directory dirents must omit modifier: {dir_part}"
    );
    assert!(
        !dir_part.contains("\"size\""),
        "directory dirents must omit size: {dir_part}"
    );
}
