use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "commits")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(not_null, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null, length = 40)]
    pub commit_id: String,
    #[sea_orm(not_null, length = 40)]
    pub root_id: String,
    pub parent_id: Option<String>,
    pub second_parent_id: Option<String>,
    #[sea_orm(not_null)]
    pub creator_name: String,
    #[sea_orm(not_null, length = 40)]
    pub creator: String,
    #[sea_orm(not_null, default_value = "")]
    pub description: String,
    #[sea_orm(not_null)]
    pub ctime: i64,
    #[sea_orm(not_null, default_value = 1)]
    pub version: i8,
    /// Set on a commit the server created by merging a client's commit with a
    /// newer HEAD (`seaf_commit_to_data`: `if (commit->new_merge) set
    /// "new_merge", 1`). NULL means "not a merge commit", which is how upstream
    /// stores it too (the field is simply absent from the JSON).
    ///
    /// The desktop client relies on it: `find_meaningful_commit()`
    /// (`daemon/sync-mgr.c`) walks past merge commits that have no conflict when
    /// deciding which commit to show in its "sync finished" notification.
    pub new_merge: Option<bool>,
    /// Set on a merge commit whose merge had to rename a conflicting entry
    /// (`opt.conflict`), i.e. a `SFConflict` file/dir was created.
    pub conflict: Option<bool>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::repo::Entity",
        from = "Column::RepoId",
        to = "super::repo::Column::Id"
    )]
    Repo,
}

impl Related<super::repo::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Repo.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
