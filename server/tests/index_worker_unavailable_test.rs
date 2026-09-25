//! What a document gets when the extraction worker cannot run at all.
//!
//! Deliberately its own integration-test binary: the worker's executable is
//! process-global and set once, and every other test's fixture points it at the
//! real binary and then insists the sandbox works. This file points it at
//! something that does not exist *before* the first fixture, which is the only
//! way to reach the unavailable path from the outside.
//!
//! The behaviour under test is the point of running documents out of process:
//! when the worker is unavailable the file is **failed**, not parsed here. The
//! backfill retries failed documents, so a host that can confine the worker
//! again picks them up; nothing in the server falls back to parsing.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use server::indexer::DocStatus;

/// A document is not indexed — and not parsed in this process — when the
/// extraction worker cannot be started.
#[tokio::test]
async fn a_document_is_not_indexed_without_the_worker() {
    assert!(
        server::indexer::extract::worker::configure_executable(PathBuf::from(
            "/nonexistent/nanofile-extract-worker"
        )),
        "this file must be the first to configure the worker"
    );

    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;
    let path = "/report.pdf";

    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "report.pdf",
            &common::minimal_pdf("zebraquartz"),
        )
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/file/reindex/", f.repo_id),
            Some(token),
            &serde_json::json!({ "p": path }),
        )
        .await;
    assert_eq!(resp.status(), 200, "the request itself must succeed");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["indexed"].as_bool(),
        Some(false),
        "a document the worker could not carry is not indexed"
    );

    // The server has to be alive and still able to index what needs no worker.
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "after.txt", b"aftermath signal")
        .await;
    assert_eq!(resp.status(), 200);
    assert!(
        wait_for(Duration::from_secs(30), || async {
            !search_results(&f, token, "aftermath").await.is_empty()
        })
        .await,
        "text needs no worker, so it must still be indexed"
    );

    // Failed, not skipped: the environment is the problem, so the retry that a
    // fixed host needs has to stay open.
    let indexer = f.server.state.indexer.clone().expect("indexer is enabled");
    let states = indexer
        .doc_states(&f.repo_id, &[path.to_string()])
        .expect("read document states");
    assert_eq!(
        states.get(path).map(|state| state.status.clone()),
        Some(DocStatus::Failed),
        "an unavailable worker is a retryable failure, not a verdict on the file"
    );
}

/// Perform a full-text content search and return the `results` array.
async fn search_results(f: &common::TestFixture, token: &str, q: &str) -> Vec<serde_json::Value> {
    let resp = f
        .client
        .get(
            &format!("/api2/search/?q={q}&search_filename_only=false"),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    resp.json::<serde_json::Value>().await.unwrap()["results"]
        .as_array()
        .unwrap()
        .clone()
}

async fn wait_for<F, Fut>(timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if predicate().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}
