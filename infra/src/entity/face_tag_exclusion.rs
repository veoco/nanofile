use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A user removed one automatic tag from one file. The scan must respect this
/// until the file's content changes.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "face_tag_exclusions")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(not_null, length = 36)]
    pub repo_id: String,
    #[sea_orm(not_null)]
    pub file_path: String,
    /// `person_any`, `person_single` or `person_group`.
    #[sea_orm(not_null, length = 64)]
    pub label_key: String,
    #[sea_orm(not_null)]
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
