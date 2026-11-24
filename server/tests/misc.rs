//! End-to-end smoke tests and server lifecycle.
//!
//! The files are modules of one test target so they link once; nextest still
//! runs every test in its own process.

#[path = "common/mod.rs"]
mod common;

mod app_test;
mod compat_contract_test;
mod integration_test;
mod repo_surface_matrix_test;
mod repos_test;
mod serve_test;
