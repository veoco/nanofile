//! The at-rest block conversion is one-shot: a converted store is skipped at
//! startup, and an interrupted pass resumes from the repository it recorded.

use crate::common;

use common::TestServer;
use infra::storage::{BlockStorageBackend, block_store::BlockStorage};
use server::AppState;
use server::fs::core::block_encryption_convert::BlockEncryptionConverter;
use server::service::upload::temp_file::TempFileManager;
use server::tasks::spec::JobKey;
use server::tasks::store::RunFilter;
use std::time::Duration;

/// The conversion walks block files, not database rows, so a block of this
/// repository is a valid fixture without creating an account or a library.
const REPO: &str = "deadbeef-0000-0000-0000-000000000001";

async fn lazy_server() -> TestServer {
    TestServer::start_with_storage_config(|storage| {
        storage.block_encryption_mode = "lazy".to_string();
        storage.encryption_key = Some("42".repeat(32));
    })
    .await
}

/// The startup pass runs on the fixture's empty store, so the marker also means
/// the job finished.
async fn wait_for_conversion(server: &TestServer) -> std::path::PathBuf {
    let marker = server.block_dir.join(".encryption_converted");
    for _ in 0..400 {
        if marker.exists() {
            return marker;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the startup block conversion did not finish");
}

fn conversion_runs() -> RunFilter {
    RunFilter {
        key: Some(JobKey::BlockEncryptionConvert),
        owner: None,
        include_active: true,
        include_terminal: true,
        limit: 0,
    }
}

#[tokio::test]
async fn a_plaintext_block_restored_after_the_pass_is_converted_on_demand() {
    let server = lazy_server().await;
    let marker = wait_for_conversion(&server).await;

    // A block restored from an older backup is exactly a plaintext file under a
    // repository directory; the pass finds it by walking the store.
    let data = b"restored before the encryption conversion".to_vec();
    let raw = BlockStorage::new(server.block_dir.clone());
    let block_id = raw.write_block(REPO, &data).await.unwrap();
    raw.invalidate_exists_cache();
    // The restored block is what the marker no longer covers.
    std::fs::remove_file(&marker).unwrap();

    let report = BlockEncryptionConverter::convert_legacy_blocks(
        &server.state.block_store,
        &server.block_dir,
        None,
    )
    .await
    .unwrap();
    assert_eq!(report.blocks_converted, 1);
    assert!(server.block_dir.join(".encryption_converted").exists());
    assert!(!server.block_dir.join(".encryption_convert.log").exists());

    // Same id, same content, versioned ciphertext on disk.
    assert_eq!(
        server
            .state
            .block_store
            .read_block(REPO, &block_id)
            .await
            .unwrap(),
        data
    );
    let stored = raw.read_block(REPO, &block_id).await.unwrap();
    assert_eq!(&stored[..4], b"NFE1");
    assert_eq!(stored.len(), data.len() + 16 + 6);

    // A marker that exists is authoritative: the next pass does not even look.
    let again = BlockEncryptionConverter::convert_legacy_blocks(
        &server.state.block_store,
        &server.block_dir,
        None,
    )
    .await
    .unwrap();
    assert!(again.already_converted);
    assert_eq!(again.blocks_checked, 0);
}

#[tokio::test]
async fn a_converted_store_does_not_submit_the_startup_pass_again() {
    let server = lazy_server().await;
    wait_for_conversion(&server).await;

    // The first start had no marker yet, so it did submit the one-shot pass.
    let first_runs = server.state.tasks.store().list(&conversion_runs());
    assert!(
        !first_runs.is_empty(),
        "the first start must submit the conversion"
    );

    // A second generation over the same data root is what a restart does; it has
    // to find the marker and submit nothing.
    let config = (*server.state.config()).clone();
    let temp = TempFileManager::new(
        config.storage.temp_dir.clone(),
        config.storage.max_temp_uploads,
        config.storage.max_temp_upload_bytes,
    )
    .await;
    let second = AppState::new((*server.db).clone(), config, temp);
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(
        second.tasks.store().list(&conversion_runs()).is_empty(),
        "a converted store must not queue the conversion again"
    );
    assert!(!server.block_dir.join(".encryption_convert.log").exists());
}
