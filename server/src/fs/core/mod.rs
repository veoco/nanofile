//! Core FS tree operations — tree traversal, FS object storage, commit+tree manipulation.
//!
//! This module was extracted from `server::repo` during the architecture refactor
//! (Phase 2). It contains the fundamental file-system operations that were mixed
//! in with repo-management code.  Repo-management-only code remains in `server::repo`.

pub mod block_encryption_convert;
pub mod download;
pub mod file_ops;
pub mod gc;
pub mod lock;
pub mod size;
pub mod store;
pub mod trash;
pub mod tree;
pub mod tree_diff;

pub use download::{Downloader, stream_blocks};
pub use file_ops::FileOps;
pub use gc::GcManager;
pub use lock::{check_commit_file_locks, upsert_lock_timestamp};
pub use size::{adjust_repo_size, compute_repo_size, compute_tree_size, get_entry_total_size};
pub use store::{store_fs_dir_object, store_fs_file_object};
pub use tree::{
    read_fs_dir_data, read_fs_dir_data_batch, read_fs_file_data, read_fs_file_data_batch,
    resolve_file_entry, resolve_file_entry_with_head, resolve_fs_id, resolve_fs_ids_batch,
};
pub use tree_diff::diff_trees;
