mod common;

use common::TestFixture;
use server::fs::core::block_migration::{BlockLayoutMigration, MigrationMode};

/// Simulate an installation that still uses the legacy flat block layout by
/// moving every block of the fixture's library back to `{block_dir}/<2hex>/<id>`
/// and dropping the layout marker.
async fn flatten_to_legacy_layout(block_dir: &std::path::Path) {
    let repos_root = block_dir.join("repos");
    let mut moved = 0usize;
    let repo_entries = std::fs::read_dir(&repos_root).expect("repos dir");
    for repo_dir in repo_entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
    {
        for prefix in std::fs::read_dir(&repo_dir)
            .expect("prefix dir")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
        {
            let target_dir = block_dir.join(prefix.file_name().unwrap());
            std::fs::create_dir_all(&target_dir).unwrap();
            for block in std::fs::read_dir(&prefix).unwrap().flatten() {
                let path = block.path();
                if !path.is_file() {
                    continue;
                }
                std::fs::rename(&path, target_dir.join(block.file_name())).unwrap();
                moved += 1;
            }
        }
    }
    assert!(moved > 0, "fixture must have produced at least one block");
    std::fs::remove_dir_all(&repos_root).unwrap();
    let _ = std::fs::remove_file(block_dir.join(".block_layout"));
}

/// A legacy installation is migrated on the spot, after which downloads work
/// again and the old tree is gone.
///
/// This is the upgrade path end to end: real repository rows, real block files,
/// the public `fs_objects` reference table and the same routine the server runs
/// before serving its first request.
#[tokio::test]
async fn legacy_layout_migrates_and_downloads_keep_working() {
    let f = TestFixture::new().await;
    let content: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "migrate.txt", &content)
        .await;
    assert!(resp.status().is_success(), "upload failed");

    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/migrate.txt")
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap().to_vec();
    assert_eq!(body, content, "baseline download must match");

    flatten_to_legacy_layout(&f.server.block_dir).await;

    // The running server has no read path for the legacy layout, so the file
    // cannot be served until the migration runs. The response status is already
    // committed when the body starts streaming, so the failure shows up as a
    // truncated body rather than a 404 — what matters here is that the content
    // is not available.
    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/migrate.txt")
        .await;
    let before_migration = match resp.bytes().await {
        Ok(bytes) => bytes.to_vec(),
        // The response declared a Content-Length it can no longer satisfy, so
        // reqwest reports an incomplete body. Either way the content is not
        // available to the client.
        Err(_) => Vec::new(),
    };
    assert_ne!(
        before_migration, content,
        "legacy-layout blocks must not be served before the migration"
    );

    let report = BlockLayoutMigration::run(
        f.server.db.as_ref(),
        &f.server.block_dir,
        MigrationMode::Apply,
    )
    .await
    .expect("migration succeeds");
    assert!(
        !report.already_migrated,
        "the fixture was flattened, so real work must happen"
    );
    assert!(report.copied > 0, "blocks must have been copied");
    assert_eq!(report.missing_sources, 0);

    // Content is back, byte for byte.
    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/migrate.txt")
        .await;
    assert_eq!(resp.status(), 200, "download after migration");
    assert_eq!(resp.bytes().await.unwrap().to_vec(), content);

    // Zero leftovers: the block root holds only the current layout.
    let mut entries: Vec<String> = std::fs::read_dir(&f.server.block_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec![".block_layout".to_string(), "repos".to_string()],
        "no legacy directory or temp file may survive the migration"
    );
    assert!(
        !BlockLayoutMigration::migration_pending(&f.server.block_dir).await,
        "nothing may be left to migrate"
    );

    // A second run is a no-op (the server does this on every startup).
    let again = BlockLayoutMigration::run(
        f.server.db.as_ref(),
        &f.server.block_dir,
        MigrationMode::Apply,
    )
    .await
    .unwrap();
    assert!(again.already_migrated);
    assert_eq!(again.copied, 0);
}

/// The dry run reports the copy volume without touching the tree, which is what
/// an operator uses to size the upgrade.
#[tokio::test]
async fn dry_run_leaves_the_legacy_layout_in_place() {
    let f = TestFixture::new().await;
    let content = b"dry run content".to_vec();
    assert!(
        f.client
            .upload_file(&f.api_token, &f.repo_id, "/", "dry.txt", &content)
            .await
            .status()
            .is_success()
    );
    flatten_to_legacy_layout(&f.server.block_dir).await;

    let report = BlockLayoutMigration::run(
        f.server.db.as_ref(),
        &f.server.block_dir,
        MigrationMode::DryRun,
    )
    .await
    .expect("dry run succeeds");

    assert!(report.dry_run);
    assert!(
        report.copied > 0,
        "the dry run reports the blocks it would copy"
    );
    assert_eq!(report.legacy_files, report.copied);
    assert!(report.legacy_bytes >= content.len() as u64);
    assert_eq!(report.purged_dirs, 0);
    assert!(
        BlockLayoutMigration::migration_pending(&f.server.block_dir).await,
        "the legacy layout must still be there after a dry run"
    );
    assert!(
        std::fs::read_dir(&f.server.block_dir)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().len() == 2),
        "legacy prefix directories must be untouched"
    );
}
