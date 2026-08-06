use crate::repository::Repositories;
use base::common::{FsDirData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use infra::entity::repo;

const SECS_PER_DAY: i64 = 86_400;

pub struct GcManager;

impl GcManager {
    /// Garbage-collect every repo that has history retention limits set.
    ///
    /// For each repo with `history_limit > 0` and/or `history_ttl_days > 0`,
    /// keeps the newest `history_limit` commits plus any commit within the
    /// last `history_ttl_days` days, then deletes unreachable fs objects and
    /// the pruned commit rows. Repos with both settings at 0 are unlimited and
    /// are left untouched.
    pub async fn garbage_collect(repos: &Repositories) -> Result<u64, AppError> {
        let now = chrono::Utc::now().timestamp();
        let all_repos = repos.repo.find_all().await?;

        let mut removed = 0u64;
        for repo_model in &all_repos {
            removed += Self::prune_repo(repos, repo_model, now).await?;
        }
        Ok(removed)
    }

    /// Prune one repo's history according to its retention settings.
    async fn prune_repo(
        repos: &Repositories,
        repo_model: &repo::Model,
        now: i64,
    ) -> Result<u64, AppError> {
        if repo_model.history_limit == 0 && repo_model.history_ttl_days == 0 {
            return Ok(0);
        }

        let commits = repos
            .commit
            .find_by_repo_id_ordered_by_ctime_desc(&repo_model.id)
            .await?;
        if commits.is_empty() {
            return Ok(0);
        }

        // Compute the set of commits to keep. The list is newest-first, so the
        // head commit is index 0 and is always retained.
        let mut keep: std::collections::HashSet<i64> = std::collections::HashSet::new();
        if repo_model.history_limit > 0 {
            for c in commits.iter().take(repo_model.history_limit as usize) {
                keep.insert(c.id);
            }
        }
        if repo_model.history_ttl_days > 0 {
            let cutoff = now - i64::from(repo_model.history_ttl_days) * SECS_PER_DAY;
            for c in &commits {
                if c.ctime >= cutoff {
                    keep.insert(c.id);
                }
            }
        }

        if keep.len() == commits.len() {
            return Ok(0);
        }

        // Collect fs ids reachable from the kept commits' roots.
        let mut active_fs_ids = std::collections::HashSet::new();
        for c in &commits {
            if keep.contains(&c.id) {
                Self::collect_fs_ids(repos, &repo_model.id, &c.root_id, &mut active_fs_ids).await?;
            }
        }

        // Delete fs objects of this repo that are no longer reachable.
        let all_fs = repos.fs_object.find_by_repo_id(&repo_model.id).await?;
        let inactive_ids: Vec<i64> = all_fs
            .iter()
            .filter(|obj| !active_fs_ids.contains(&obj.fs_id))
            .map(|obj| obj.id)
            .collect();
        let removed = inactive_ids.len();
        if !inactive_ids.is_empty() {
            repos.fs_object.delete_many_by_ids(inactive_ids).await?;
        }

        // Delete the pruned commit rows.
        let pruned_ids: Vec<i64> = commits
            .iter()
            .filter(|c| !keep.contains(&c.id))
            .map(|c| c.id)
            .collect();
        if !pruned_ids.is_empty() {
            repos.commit.delete_many_by_ids(pruned_ids).await?;
        }

        Ok(removed as u64)
    }

    /// Breadth-first walk from `root_id`, adding every reachable fs_id to
    /// `collected`. Directory objects are fetched level by level in a single
    /// batched query instead of one query per object.
    async fn collect_fs_ids(
        repos: &Repositories,
        repo_id: &str,
        root_id: &str,
        collected: &mut std::collections::HashSet<String>,
    ) -> Result<(), AppError> {
        if collected.contains(root_id) {
            return Ok(());
        }
        collected.insert(root_id.to_string());

        let mut frontier = vec![root_id.to_string()];
        while !frontier.is_empty() {
            let objs = repos
                .fs_object
                .find_by_repo_and_fs_ids(repo_id, &frontier)
                .await?;
            let mut next = Vec::new();
            for obj in &objs {
                if obj.obj_type == SEAF_METADATA_TYPE_DIR as i8 {
                    let dir_data: FsDirData = serde_json::from_str(&obj.data)
                        .map_err(|e| AppError::internal(e.to_string()))?;
                    for entry in &dir_data.dirents {
                        if collected.insert(entry.id.clone()) {
                            next.push(entry.id.clone());
                        }
                    }
                }
            }
            frontier = next;
        }

        Ok(())
    }
}
