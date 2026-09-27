//! What the confined child does with a document at the size the pipeline allows.
//!
//! Every other sandbox test uses a document of a few hundred bytes, so the
//! resource limits the child installs are measured as *present* and never as
//! *sufficient*: address space, CPU time, descriptors and file size could all be
//! clamped to values no real document survives and the suite would stay green
//! while every large document became an unparsable file in production.
//!
//! The two cases below put the documented sizes to the running limit. A
//! document whose text is most of the way to `MAX_INDEXED_CONTENT_BYTES` has to
//! come back whole — the marker at its end is what says the parse was not cut
//! short — and one past the cap has to come back cut exactly at the cap.
//!
//! Its own integration-test binary because the worker's executable is
//! process-global and set once: this file points it at the real binary before
//! any probe, and no fixture is built here.

mod common;

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use server::indexer::extract::{Document, Extracted, MAX_INDEXED_CONTENT_BYTES, Plan};
use server::sandbox::worker::{self, Outcome};
use server::sandbox::{Level, Profile};

/// How much text the large document carries.
///
/// Three quarters of the cap the child truncates at: large enough that a limit
/// which is merely *installed* cannot satisfy it, with room below the cap so a
/// whole-text comparison is what the case measures rather than the truncation
/// rule (which the second case covers).
const LARGE_TEXT_BYTES: usize = 6 * 1024 * 1024;

/// A word no filler repeats, placed where truncation would drop it.
const MARKER: &str = "zebramarker";

/// Point the worker at this package's own binary, once, before any probe.
fn the_worker_ran() {
    static CONFIGURED: OnceLock<()> = OnceLock::new();
    CONFIGURED.get_or_init(|| {
        assert!(
            common::sandbox::configure_real_worker(),
            "this file must be the first to configure the worker"
        );
    });
}

/// The child has to confine itself on this host, and the failure has to say
/// what the host gave instead.
fn require_confinement() {
    common::sandbox::require_level(Profile::Documents, Level::Partial);
}

/// A PDF whose one text object draws `text`.
fn pdf_with_text(text: &str) -> Vec<u8> {
    common::minimal_pdf_with_stream(&format!("BT /F1 24 Tf 72 720 Td ({text}) Tj ET"))
}

/// Extract `pdf` in the confined child, or panic with what came back instead.
fn extracted(pdf: Vec<u8>) -> String {
    match worker::extract(Plan::Document(Document::Pdf), pdf) {
        Outcome::Extracted(Extracted::Text(text)) => text,
        Outcome::Extracted(Extracted::Unsupported(reason)) => {
            panic!("the confined child refused the document: {reason}")
        }
        Outcome::Unavailable(why) => panic!("the confined child could not run: {why}"),
        Outcome::Failed(why) => panic!("the confined child did not answer: {why}"),
    }
}

/// A document near the text cap comes back whole, in the time the parent allows.
#[test]
fn a_document_near_the_text_cap_is_extracted_whole() {
    the_worker_ran();
    require_confinement();

    let text = format!("{}{MARKER}", "a".repeat(LARGE_TEXT_BYTES));

    let started = Instant::now();
    let extracted = extracted(pdf_with_text(&text));
    let elapsed = started.elapsed();

    assert_eq!(
        extracted.trim(),
        text,
        "the whole text has to survive the confinement, marker and all"
    );
    // The parent kills a request at `worker::TIMEOUT` (20s) and the child's own
    // CPU clamp is 15s. A document at the size the pipeline documents has to
    // finish with room to spare, or the limits are too tight to be usable.
    assert!(
        elapsed < Duration::from_secs(15),
        "a 6 MiB document took {elapsed:?} through the confined child"
    );
    eprintln!("a {LARGE_TEXT_BYTES}-byte document took {elapsed:?} through the confined child");
}

/// Text past the cap is cut exactly at the cap, in the child rather than here.
#[test]
fn text_past_the_cap_is_cut_exactly_at_the_cap() {
    the_worker_ran();
    require_confinement();

    let text = format!(
        "{}{MARKER}",
        "a".repeat(MAX_INDEXED_CONTENT_BYTES + 512 * 1024)
    );
    let extracted = extracted(pdf_with_text(&text));

    assert_eq!(
        extracted.len(),
        MAX_INDEXED_CONTENT_BYTES,
        "the cap is the length the child keeps"
    );
    assert!(
        !extracted.contains(MARKER),
        "the text past the cap must not be kept"
    );
}
