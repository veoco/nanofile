//! Core FS tree operations — tree traversal, FS object storage, commit+tree manipulation.
//!
//! This module was extracted from `server::repo` during the architecture refactor
//! (Phase 2). It contains the fundamental file-system operations that were mixed
//! in with repo-management code.  Repo-management-only code remains in `server::repo`.

pub mod download;
pub mod file_ops;
pub mod gc;
pub mod size;
pub mod store;
pub mod trash;
pub mod tree;
pub mod tree_diff;
pub mod versioning;

pub use download::Downloader;
pub use file_ops::FileOps;
pub use gc::GcManager;
pub use size::{adjust_repo_size, compute_repo_size, compute_tree_size, get_entry_total_size};
pub use store::{store_fs_dir_object, store_fs_file_object};
pub use tree::{read_fs_dir_data, read_fs_file_data, resolve_fs_id};
pub use tree_diff::diff_trees;
pub use versioning::Versioning;
