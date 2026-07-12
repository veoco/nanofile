// Re-exports for backward compatibility — types now live in crate::service::repo::repo.
pub use crate::service::repo::service::{
    LeftPanelRepo, RepoInfo, V21RepoInfo, V21RepoListResponse, load_left_panel_repos,
};
