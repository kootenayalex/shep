//! Task-queue API types (M4). The queue itself lives in `<state dir>/tasks.db`
//! and is edited directly by `shep task add/list/cancel`; only dispatch — which
//! spawns panes — goes through the server.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct TaskDispatchParams {
    /// Task to dispatch; omitted = oldest queued task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<i64>,
}
