//! Block-integrity regressions for at-rest encrypted libraries.
//!
//! An offset-based client (and the server's own `Range` fast path) derives byte
//! offsets from the block sizes it is told. Those sizes must be the *logical*
//! ones for both on-disk at-rest formats — versioned ciphertext (6-byte `NFE1`
//! header + 16-byte tag) and the header-less legacy format (tag only). Sizing
//! every block as if it were versioned under-reports each legacy block by six
//! bytes, which shifts every byte after it.

mod common;

use base::common::FsFileData;
use common::{TestServer, client, create_test_repo, create_test_user, get_sync_token};

/// Deterministic pseudo-random bytes; only the lengths matter here.
fn pseudo_block(seed: u64, len: usize) -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0x9E37_79B9);
    (0..len)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 33) as u8
        })
        .collect()
}

/// Upload `blocks` as separate content-addressed blocks and store one file
/// object referencing them in order. Returns `(file_id, block_ids)`.
async fn put_multiblock_file(
    client: &client::TestClient,
    token: &str,
    repo_id: &str,
    blocks: &[Vec<u8>],
) -> (String, Vec<String>) {
    let mut block_ids = Vec::new();
    let mut total = 0i64;
    for block in blocks {
        let id = infra::crypto::fs_id::sha1_hex(block);
        let resp = client.put_block(token, repo_id, &id, block.clone()).await;
        assert_eq!(resp.status(), 200, "put_block failed");
        block_ids.push(id);
        total += block.len() as i64;
    }

    let fs = FsFileData {
        block_ids: block_ids.clone(),
        size: total,
        obj_type: 1,
        version: 1,
    };
    let json = serde_json::to_string(&fs).unwrap();
    let id = infra::crypto::fs_id::sha1_hex(json.as_bytes());
    let packed = infra::serialization::pack_fs::compress_fs_data(json.as_bytes()).unwrap();
    let mut body = Vec::new();
    body.extend_from_slice(id.as_bytes());
    body.extend_from_slice(&(packed.len() as u32).to_be_bytes());
    body.extend_from_slice(&packed);

    let resp = client.recv_fs(token, repo_id, body).await;
    let status = resp.status();
    if !status.is_success() {
        panic!(
            "recv_fs for file object failed: {status} {}",
            resp.text().await.unwrap_or_default()
        );
    }
    (id, block_ids)
}

/// Rewrite a stored block in the pre-`NFE1` header-less format, i.e. what a
/// deployment that encrypted blocks before the versioned header existed left on
/// disk. Reads go to disk, so the running server sees the change.
fn strip_versioned_header(block_dir: &std::path::Path, repo_id: &str, block_id: &str) {
    let path = block_dir
        .join("repos")
        .join(infra::crypto::fs_id::sha1_hex(repo_id.as_bytes()))
        .join(&block_id[..2])
        .join(block_id);
    let stored = std::fs::read(&path).expect("block file must exist");
    assert_eq!(&stored[..4], b"NFE1", "fixture must start versioned");
    std::fs::write(&path, &stored[6..]).expect("rewrite block without its header");
}

/// `block-map` must report logical sizes for header-less legacy blocks too, and
/// the sizes must add up to the file object's declared size. Clients slice
/// their downloads (and their resume point) from exactly these numbers.
#[tokio::test]
async fn block_map_reports_logical_sizes_for_headerless_at_rest_blocks() {
    let server = TestServer::start_with_storage_config(|storage| {
        storage.block_encryption_mode = "on".to_string();
        // 64 hex chars decode to the 32-byte master key.
        storage.encryption_key = Some("42".repeat(32));
    })
    .await;

    let client = server.client();
    let db = &*server.db;
    create_test_user(db, "test@example.com", "password").await;

    let resp = client.login("test@example.com", "password").await;
    assert_eq!(resp.status(), 200, "login failed");
    let api_token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let repo_id = create_test_repo(&client, &api_token, "test-repo").await;
    let sync_token = get_sync_token(&client, &api_token, &repo_id).await;

    let blocks: Vec<Vec<u8>> = [4096usize, 8192, 8192, 2048]
        .iter()
        .enumerate()
        .map(|(seed, len)| pseudo_block(seed as u64 + 1, *len))
        .collect();
    let (file_id, block_ids) = put_multiblock_file(&client, &sync_token, &repo_id, &blocks).await;

    // The first three blocks predate the versioned header — the mix the
    // reported library has, and what made three skipped blocks an 18-byte shift.
    for id in &block_ids[..3] {
        strip_versioned_header(&server.block_dir, &repo_id, id);
    }

    let resp = client.block_map(&sync_token, &repo_id, &file_id).await;
    assert_eq!(resp.status(), 200, "block-map failed");
    let sizes: Vec<i64> = resp.json().await.unwrap();
    let expected: Vec<i64> = blocks.iter().map(|b| b.len() as i64).collect();
    assert_eq!(
        sizes, expected,
        "block-map must report each block's logical size, whatever its at-rest format"
    );
    assert_eq!(
        sizes.iter().sum::<i64>(),
        expected.iter().sum::<i64>(),
        "block-map sizes must add up to the declared size"
    );
}
