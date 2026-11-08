//! Sync protocol and tokens.
//!
//! The files are modules of one test target so they link once; nextest still
//! runs every test in its own process.

#[path = "common/mod.rs"]
mod common;

mod sync_aux_test;
mod sync_flow_test;
mod sync_merge_test;
mod sync_quota_test;
mod sync_test;
mod sync_token_device_test;
