use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "activities")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(not_null, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null, length = 40)]
    pub commit_id: String,
    #[sea_orm(not_null)]
    pub op_type: String,
    #[sea_orm(not_null)]
    pub obj_type: String,
    #[sea_orm(not_null)]
    pub path: String,
    pub old_path: Option<String>,
    #[sea_orm(not_null)]
    pub user_id: i32,
    #[sea_orm(not_null)]
    pub created_at: i64,
    #[sea_orm(not_null)]
    pub detail: String,
    /// Number of objects in this activity. Zero for rows written before the
    /// `activity_items` split; readers then fall back to `detail`.
    #[sea_orm(not_null)]
    pub item_count: i32,
    /// What produced the row: `session` for a single call that the 5-minute
    /// aggregation may append to, `commit` for a sync commit group. Keeping the
    /// two apart prevents a Web upload from appending to a commit's summary.
    #[sea_orm(not_null, length = 16)]
    pub group_kind: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
