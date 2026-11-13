use sea_orm::entity::prelude::*;

/// One object changed by an activity. The summary row (`activities`) carries the
/// operation, author and timestamp; these rows carry the paths, sizes and the
/// server-merge markers. Splitting them out keeps one large upload from bloating
/// the summary row and makes the exact item count independent of the returned
/// detail slice.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "activity_items")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(not_null)]
    pub activity_id: i32,
    #[sea_orm(not_null)]
    pub path: String,
    pub old_path: Option<String>,
    pub size: Option<i64>,
    #[sea_orm(length = 40)]
    pub obj_id: Option<String>,
    #[sea_orm(not_null)]
    pub merge: bool,
    #[sea_orm(not_null)]
    pub merge_kept: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::activity::Entity",
        from = "Column::ActivityId",
        to = "super::activity::Column::Id"
    )]
    Activity,
}

impl Related<super::activity::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Activity.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
