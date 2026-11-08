//! Full-text index and the ignored performance probes.
//!
//! The files are modules of one test target so they link once; nextest still
//! runs every test in its own process.

#[path = "common/mod.rs"]
mod common;

mod index_test;
mod perf_probe;
