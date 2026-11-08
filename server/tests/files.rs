//! Filesystem objects, uploads, downloads and trash.
//!
//! The files are modules of one test target so they link once; nextest still
//! runs every test in its own process.

#[path = "common/mod.rs"]
mod common;

mod block_integrity_test;
mod block_layout_test;
mod chunked_upload_test;
mod commit_test;
mod dir_order_test;
mod encrypted_repo_test;
mod file_detail_test;
mod fileops_test;
mod fs_id_compat;
mod fs_test;
mod lock_file_test;
mod quota_accounting_test;
mod quota_test;
mod repo_size_test;
mod repo_trash_test;
mod trash_restore_test;
mod upload_link_test;
mod webdav_test;
mod win_reserved_name_test;
mod zip_download_test;
