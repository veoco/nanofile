//! What a document gets while `sandbox.enabled` is off, and what it gets when
//! the switch is turned back on.
//!
//! Its own integration-test binary because the sandbox requirement is
//! process-global and the fixture sets it at startup: this file changes it
//! after the fixture is built, so no other test may share the process.
//!
//! The behaviour under test is that the switch is reversible: a document
//! skipped while it is off is recorded as a *retryable* state rather than as
//! "this file is not indexable", so turning the sandbox back on re-indexes it
//! instead of leaving the document missing from search forever.

mod common;

use server::indexer::DocStatus;
use server::sandbox::{Level, Requirement, worker};

/// A document that meets the switched-off sandbox is a retryable failure, and
/// the same document is indexed once the sandbox is back.
#[tokio::test]
async fn a_document_skipped_while_the_sandbox_is_off_is_reindexed_when_it_returns() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;
    let path = "/sandbox-off.pdf";

    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "sandbox-off.pdf",
            &common::minimal_pdf("zebraquartz"),
        )
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    // The master switch, off: nothing is parsed, in the child or here.
    worker::configure_requirement(Requirement::new(false, Level::None));

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
        "a document is not indexed while the sandbox is off"
    );

    let indexer = f.server.state.indexer.clone().expect("indexer is enabled");
    // The state is written asynchronously, so give it the same bounded wait a
    // backfill would.
    let mut status = None;
    for _ in 0..200 {
        let states = indexer
            .doc_states(&f.repo_id, &[path.to_string()])
            .expect("read document states");
        status = states.get(path).map(|state| state.status.clone());
        if status.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        status,
        Some(DocStatus::Failed),
        "the switched-off sandbox must leave a retryable state, not `Skipped`"
    );

    // The switch back on: the same bytes are extracted for real. The level is
    // the shipped default, and this host is expected to reach it.
    worker::configure_requirement(Requirement::new(true, server::sandbox::Level::Partial));

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
        Some(true),
        "the document must be indexed once the sandbox is back: {body}"
    );
}
