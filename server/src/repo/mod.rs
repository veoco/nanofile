pub mod fs_tree;
pub mod handler;
pub mod service;
pub mod store;

// Re-export service modules for backward compatibility
pub use service::download;
pub use service::file_ops;
pub use service::gc;
pub use service::size;
pub use service::trash;
pub use service::tree_diff;
pub use service::versioning;

pub use fs_tree::{read_fs_dir_data, read_fs_file_data, resolve_fs_id};
pub use service::download::Downloader;
pub use service::file_ops::FileOps;
pub use service::gc::GcManager;
pub use service::repo::{
    LeftPanelRepo, RepoInfo, V21RepoInfo, V21RepoListResponse, load_left_panel_repos,
};
pub use service::size::{
    adjust_repo_size, compute_repo_size, compute_tree_size, get_entry_total_size,
};
pub use service::tree_diff::diff_trees;
pub use service::versioning::Versioning;
pub use store::{store_fs_dir_object, store_fs_file_object};
