use sea_orm::DatabaseConnection;

use crate::error::AppError;

mod reindex;
mod users;

pub use reindex::AdminService;
pub use users::AdminUserService;

/// Collect all file paths from a FS tree recursively.
pub(crate) async fn collect_file_paths(
    db: &DatabaseConnection,
    repo_id: &str,
    root_fs_id: &str,
) -> Result<Vec<String>, AppError> {
    use crate::common::EMPTY_SHA1;
    use crate::serialization::S_IFDIR;

    let mut results = Vec::new();
    let mut stack = vec![(root_fs_id.to_string(), String::new())];

    while let Some((fs_id, path)) = stack.pop() {
        if fs_id == EMPTY_SHA1 {
            continue;
        }

        let dir_data = match crate::repo::read_fs_dir_data(db, repo_id, &fs_id).await {
            Ok(data) => data,
            Err(_) => continue,
        };

        for entry in &dir_data.dirents {
            let full_path = if path.is_empty() {
                format!("/{}", entry.name)
            } else if path.starts_with('/') {
                format!("{}/{}", path, entry.name)
            } else {
                format!("/{}/{}", path, entry.name)
            };

            if entry.mode & S_IFDIR != 0 {
                stack.push((entry.id.clone(), full_path));
            } else {
                results.push(full_path);
            }
        }
    }

    Ok(results)
}
