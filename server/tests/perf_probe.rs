//! Manual performance/memory probe for the hot paths touched by the
//! performance work. These tests are `#[ignore]`d: they take seconds and their
//! numbers depend on the machine, so CI should never assert on them.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p server --release --test perf_probe -- --ignored --nocapture
//! ```
//!
//! `PERF_FILES` overrides the number of files the HTTP scenario creates
//! (default 200). The output is one `perf <name> ...` line per scenario so a
//! before/after run can be diffed.

mod common;

use std::time::{Duration, Instant};

use common::TestFixture;
use infra::storage::BlockStorageBackend;

/// Resident set size in KiB on Linux, `None` elsewhere.
fn rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let rest = line.strip_prefix("VmRSS:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

fn report(name: &str, elapsed: Duration, units: u64, unit: &str) {
    let seconds = elapsed.as_secs_f64();
    let per_second = if seconds > 0.0 {
        units as f64 / seconds
    } else {
        0.0
    };
    println!(
        "perf {name}: {} ms, {units} {unit} ({per_second:.0}/s), rss={}",
        elapsed.as_millis(),
        rss_kib().map_or("n/a".to_string(), |kib| format!("{kib} KiB"))
    );
}

/// Block-store presence checks: the cache key changed from an allocated
/// `String` to a fixed 40-byte digest, so this measures the lookup path.
#[tokio::test]
#[ignore = "manual performance probe"]
async fn probe_block_existence_cache() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = infra::storage::block_store::BlockStorage::new(dir.path().to_path_buf());
    let repo = "probe-repo";

    let mut ids = Vec::new();
    for i in 0..500u32 {
        let data = format!("block-{i}");
        ids.push(
            store
                .write_block(repo, data.as_bytes())
                .await
                .expect("write"),
        );
    }

    let start = Instant::now();
    let mut found = 0usize;
    for _ in 0..200 {
        for id in &ids {
            if store.has_block(repo, id).await {
                found += 1;
            }
        }
    }
    assert_eq!(found, 100_000, "every cached block must report present");
    report(
        "block_existence_cache",
        start.elapsed(),
        (ids.len() * 200) as u64,
        "lookups",
    );
}

/// CDC chunking throughput, independent of disk and HTTP.
#[test]
#[ignore = "manual performance probe"]
fn probe_cdc_throughput() {
    const TOTAL: usize = 64 * 1024 * 1024;
    let data: Vec<u8> = (0..TOTAL).map(|i| (i * 31 % 251) as u8).collect();

    let start = Instant::now();
    let mut chunker = infra::storage::cdc::Chunker::new(TOTAL);
    let mut blocks = 0u64;
    for chunk in data.chunks(64 * 1024) {
        blocks += chunker.feed(chunk).len() as u64;
    }
    blocks += u64::from(!chunker.finish().is_empty());
    assert!(blocks > 0);
    report("cdc_64mib", start.elapsed(), TOTAL as u64, "bytes");
}

/// PBKDF2 at the default iteration count, through the async wrapper that the
/// request paths now use.
#[tokio::test]
#[ignore = "manual performance probe"]
async fn probe_password_hash() {
    let start = Instant::now();
    let hash =
        server::service::auth::password::hash_password_async("probe-password".to_string(), 600_000)
            .await;
    let hashed = start.elapsed();
    assert!(hash.starts_with("pbkdf2_sha256$600000$"));
    report("pbkdf2_600k", hashed, 1, "hash");

    let start = Instant::now();
    let ok = server::service::auth::password::verify_password_async(
        "probe-password".to_string(),
        hash,
        600_000,
    )
    .await;
    assert!(ok);
    report("pbkdf2_verify_600k", start.elapsed(), 1, "verify");
}

/// End-to-end HTTP scenario: upload N files, list the directory, make history
/// on one file and read it back. This is the closest cheap proxy for the web UI
/// and sync listing paths.
#[tokio::test]
#[ignore = "manual performance probe"]
async fn probe_http_listing_and_history() {
    let files: usize = std::env::var("PERF_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(200);
    assert!(files <= 5_000, "keep the probe bounded");

    let f = TestFixture::new().await;
    let rss_before = rss_kib();

    let body = vec![b'x'; 64 * 1024];
    let start = Instant::now();
    for i in 0..files {
        let resp = f
            .client
            .upload_file(
                &f.api_token,
                &f.repo_id,
                "/",
                &format!("perf-{i:05}.bin"),
                &body,
            )
            .await;
        assert_eq!(resp.status(), 200, "upload {i} failed");
    }
    report("http_upload_64k", start.elapsed(), files as u64, "files");

    let start = Instant::now();
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let entries = resp.json::<serde_json::Value>().await.unwrap();
    assert_eq!(entries.as_array().unwrap().len(), files);
    report("http_list_dir", start.elapsed(), files as u64, "entries");

    // Replace one file repeatedly, then read its history: this exercises the
    // commit walk and the `(repo_id, ctime, id)` index.
    for i in 0..50 {
        let data = format!("version-{i}");
        let resp = f
            .client
            .upload_file(
                &f.api_token,
                &f.repo_id,
                "/",
                "perf-00000.bin",
                data.as_bytes(),
            )
            .await;
        assert_eq!(resp.status(), 200, "replace {i} failed");
    }
    let start = Instant::now();
    let resp = f
        .client
        .get(
            &format!(
                "/api/v2.1/repos/{}/file/history/?p=/perf-00000.bin&per_page=50",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    report("http_file_history", start.elapsed(), 50, "versions");

    if let (Some(before), Some(after)) = (rss_before, rss_kib()) {
        println!("perf http_scenario_rss: before={before} KiB after={after} KiB");
    }
}
