//! The child tasks a `split` Step filed: when a Run's plan finds its task
//! too big for one Run, the Step files each child as the Run's owner and
//! keeps its id and name, so the Run's read lists them without asking
//! document storage.

use serde::Serialize;
use uuid::Uuid;

/// A child task a `split` Step filed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SplitTask {
    /// The `split` Step that filed it.
    pub seq: u16,
    /// Its place in the plan's children, from 0.
    pub place: u16,
    /// The task document's id.
    pub task_id: Uuid,
    /// Its name, as the plan named it.
    pub name: String,
}
