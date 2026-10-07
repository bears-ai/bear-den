//! Den-owned session access projections and expected Work startup identity.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Advisory client projection. Den rechecks canonical authority at each effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionAccessState {
    AwaitingHat,
    Executable,
    ReadOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionAccess {
    pub state: SessionAccessState,
    pub may_select_hat: bool,
}

/// The checkout identity a headless client expects to execute, never a grant.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedWorkSource {
    pub work_run_id: Uuid,
    pub execution_attempt_id: Uuid,
    pub fence_epoch: i64,
}
