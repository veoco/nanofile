use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-image result of the YuNet scan. A row exists even when no face was found,
/// so the backfill can distinguish "already looked at" from "never seen".
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "face_tag_scans")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(not_null, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null)]
    pub file_path: String,
    #[sea_orm(not_null, length = 40)]
    pub fs_id: String,
    #[sea_orm(not_null)]
    pub model_version: i32,
    /// `indexed`, `skipped` or `failed`.
    #[sea_orm(not_null, length = 16)]
    pub status: String,
    pub reason: Option<String>,
    #[sea_orm(not_null, default = 0)]
    pub face_count: i32,
    pub max_score: Option<f32>,
    #[sea_orm(not_null)]
    pub attempted_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
