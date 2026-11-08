//! Server administration and settings.
//!
//! The files are modules of one test target so they link once; nextest still
//! runs every test in its own process.

#[path = "common/mod.rs"]
mod common;

mod admintasks_test;
mod settings_test;
mod switches_test;
