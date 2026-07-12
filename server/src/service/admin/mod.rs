use crate::repository::Repositories;
use base::error::AppError;
use infra::common::EMPTY_SHA1;
use infra::serialization::S_IFDIR;

pub mod reindex;
pub mod users;

pub use reindex::AdminService;
pub use users::AdminUserService;

/// Collect all file paths from a FS tree recursively.
pub(crate) async fn collect_file_paths(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
) -> Result<Vec<String>, AppError> {
    let mut results = Vec::new();
    let mut stack = vec![(root_fs_id.to_string(), String::new())];
    while let Some((current_id, prefix)) = stack.pop() {
        if current_id == EMPTY_SHA1 {
            continue;
        }
        let dir_data = crate::fs::core::read_fs_dir_data(repos, repo_id, &current_id).await?;
        for entry in &dir_data.dirents {
            let path = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{}/{}", prefix, entry.name)
            };
            if entry.mode & S_IFDIR != 0 {
                stack.push((entry.id.clone(), path));
            } else {
                results.push(path);
            }
        }
    }
    Ok(results)
}
