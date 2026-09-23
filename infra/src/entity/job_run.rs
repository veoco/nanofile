use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A durable record of one background run.
///
/// The in-memory run table keeps what is happening now; this keeps what
/// happened, across a crash or a power cut. It is deliberately *not* a second
/// source of truth while the process is alive.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "job_runs")]
pub struct Model {
    /// The run's id, as handed to whoever submitted it.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// Stable job slug.
    #[sea_orm(not_null)]
    pub kind: String,
    /// Submitting account, when the job belongs to one.
    #[sea_orm(nullable)]
    pub owner: Option<i32>,
    /// `queued` | `running` | `yielded` | `succeeded` | `failed` | `cancelled`
    /// | `timed_out` | `interrupted`.
    #[sea_orm(not_null)]
    pub phase: String,
    /// Human summary, so an administrator can tell two runs apart.
    #[sea_orm(not_null, default_value = "")]
    pub summary: String,
    /// The submit-time input, kept only while the run may still be resumed.
    #[sea_orm(nullable)]
    pub params: Option<String>,
    /// Error text of a failed run.
    #[sea_orm(nullable)]
    pub error: Option<String>,
    /// Items processed, for the aggregate columns on the task page.
    #[sea_orm(nullable)]
    pub processed: Option<i64>,
    /// Attempt number, incremented by recovery.
    #[sea_orm(not_null, default_value = 1)]
    pub attempt: i32,
    #[sea_orm(not_null, default_value = 0)]
    pub created_at: i64,
    #[sea_orm(nullable)]
    pub started_at: Option<i64>,
    #[sea_orm(nullable)]
    pub finished_at: Option<i64>,
    /// When a recovery pass may take this row over. `None` for a finished row,
    /// or one nobody is holding.
    #[sea_orm(nullable)]
    pub lease_until: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
