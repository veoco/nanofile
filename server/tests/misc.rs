//! End-to-end smoke tests and server lifecycle.
//!
//! The files are modules of one test target so they link once; nextest still
//! runs every test in its own process.

#[path = "common/mod.rs"]
mod common;

mod app_test;
mod integration_test;
mod repos_test;
mod serve_test;
