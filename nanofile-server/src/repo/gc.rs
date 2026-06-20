use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Statement,
};

use crate::entity::{commit, fs_object};

pub struct GcManager;

impl GcManager {
    pub async fn garbage_collect(
        db: &DatabaseConnection,
        keep_commits: u64,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        let all_fs_ids = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.ne(""))
            .all(db)
            .await?;

        let mut active_fs_ids = std::collections::HashSet::new();

        let commits = commit::Entity::find()
            .order_by_desc(commit::Column::Ctime)
            .all(db)
            .await?;

        let mut commits_to_check = Vec::new();
        for c in &commits {
            if commits_to_check.len() < keep_commits as usize {
                commits_to_check.push(c);
            }
        }

        for c in &commits_to_check {
            Self::collect_fs_ids(db, &c.root_id, &mut active_fs_ids).await?;
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
            let id_list = inactive_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            db.execute(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!("DELETE FROM fs_objects WHERE id IN ({id_list})"),
            ))
            .await?;
        }

        Ok(removed)
    }

    async fn collect_fs_ids(
        db: &DatabaseConnection,
        fs_id: &str,
        collected: &mut std::collections::HashSet<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if collected.contains(fs_id) {
            return Ok(());
        }

        collected.insert(fs_id.to_string());

        let fs_obj = fs_object::Entity::find()
            .filter(fs_object::Column::FsId.eq(fs_id))
            .one(db)
            .await?;

        if let Some(obj) = fs_obj
            && obj.obj_type == 3
        {
            let dir_data: crate::serialization::fs_json::FsDirData =
                serde_json::from_str(&obj.data)?;
            for entry in &dir_data.dirents {
                Box::pin(Self::collect_fs_ids(db, &entry.id, collected)).await?;
            }
        }

        Ok(())
    }
}
