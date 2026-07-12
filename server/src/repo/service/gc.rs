use crate::error::AppError;
use crate::repository::Repositories;

pub struct GcManager;

impl GcManager {
    pub async fn garbage_collect(repos: &Repositories, keep_commits: u64) -> Result<u64, AppError> {
        let all_fs_ids = repos.fs_object.find_all().await?;

        let mut active_fs_ids = std::collections::HashSet::new();

        let commits = repos.commit.find_all_ordered_by_ctime_desc().await?;

        let mut commits_to_check = Vec::new();
        for c in &commits {
            if commits_to_check.len() < keep_commits as usize {
                commits_to_check.push(c);
            }
        }

        for c in &commits_to_check {
            Self::collect_fs_ids(repos, &c.root_id, &mut active_fs_ids).await?;
        }

        // Batch delete inactive objects with a single SQL statement
        // instead of N individual delete_by_id calls.
        let inactive_ids: Vec<i64> = all_fs_ids
            .iter()
            .filter(|fs_obj| !active_fs_ids.contains(&fs_obj.fs_id))
            .map(|fs_obj| fs_obj.id)
            .collect();

        let removed = inactive_ids.len() as u64;

        if !inactive_ids.is_empty() {
            repos.fs_object.delete_many_by_ids(inactive_ids).await?;
        }

        Ok(removed)
    }

    async fn collect_fs_ids(
        repos: &Repositories,
        fs_id: &str,
        collected: &mut std::collections::HashSet<String>,
    ) -> Result<(), AppError> {
        if collected.contains(fs_id) {
            return Ok(());
        }

        collected.insert(fs_id.to_string());

        let fs_obj = repos.fs_object.find_by_fs_id(fs_id).await?;

        if let Some(obj) = fs_obj
            && obj.obj_type == 3
        {
            let dir_data: base::common::FsDirData =
                serde_json::from_str(&obj.data).map_err(|e| AppError::internal(e.to_string()))?;
            for entry in &dir_data.dirents {
                Box::pin(Self::collect_fs_ids(repos, &entry.id, collected)).await?;
            }
        }

        Ok(())
    }
}
