use crate::repository::Repositories;
use base::error::AppError;
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
    let mut frontier = vec![(root_fs_id.to_string(), String::new())];
    while !frontier.is_empty() {
        let ids: Vec<String> = frontier.iter().map(|(id, _)| id.clone()).collect();
        let dir_map = crate::fs::core::read_fs_dir_data_batch(repos, repo_id, &ids).await?;

        let mut next = Vec::new();
        for (current_id, prefix) in frontier {
            let Some(dir_data) = dir_map.get(&current_id) else {
                continue; // EMPTY_SHA1 or missing directory
            };
            for entry in &dir_data.dirents {
                let path = if prefix.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{}/{}", prefix, entry.name)
                };
                if entry.mode & S_IFDIR != 0 {
                    next.push((entry.id.clone(), path));
                } else {
                    results.push(path);
                }
            }
        }
        frontier = next;
    }
    Ok(results)
}
