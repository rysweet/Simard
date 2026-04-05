//! Data types for goal assignment and progress tracking.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::error::SimardResult;
use crate::session::SessionPhase;

/// Progress report from a subordinate agent.
///
/// Written as a semantic fact so the supervisor can poll it from the hive.
/// Serialized to JSON for storage in the fact's `content` field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubordinateProgress {
    /// The subordinate's unique identifier.
    pub sub_id: String,
    /// Current session phase of the subordinate.
    pub phase: String,
    /// Number of steps completed so far.
    pub steps_completed: u32,
    /// Total expected steps (0 if unknown).
    pub steps_total: u32,
    /// Description of the last action taken.
    pub last_action: String,
    /// Unix epoch seconds of the last heartbeat.
    pub heartbeat_epoch: u64,
    /// Final outcome, if the goal is complete.
    pub outcome: Option<String>,
}

impl Display for SubordinateProgress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SubordinateProgress(sub={}, phase={}, {}/{}, last={})",
            self.sub_id, self.phase, self.steps_completed, self.steps_total, self.last_action
        )
    }
}

impl SubordinateProgress {
    /// Create a new progress report at the current time.
    pub fn new(
        sub_id: impl Into<String>,
        phase: SessionPhase,
        steps_completed: u32,
        steps_total: u32,
        last_action: impl Into<String>,
    ) -> SimardResult<Self> {
        let epoch = crate::metadata::Freshness::now()?.observed_at_unix_ms / 1000;
        Ok(Self {
            sub_id: sub_id.into(),
            phase: phase.to_string(),
            steps_completed,
            steps_total,
            last_action: last_action.into(),
            heartbeat_epoch: epoch,
            outcome: None,
        })
    }

    /// Attach a final outcome to this progress report.
    pub fn with_outcome(mut self, outcome: impl Into<String>) -> Self {
        self.outcome = Some(outcome.into());
        self
    }
}
