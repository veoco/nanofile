//! People tags, thumbnails and model assets.
//!
//! The files are modules of one test target so they link once; nextest still
//! runs every test in its own process.

#[path = "common/mod.rs"]
mod common;

mod face_tags_repository_test;
mod face_tags_test;
mod model_assets_test;
mod thumbnail_sandbox_test;
mod thumbnail_test;
