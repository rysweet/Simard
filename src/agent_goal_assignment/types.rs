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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subordinate_progress_display() {
        let p = SubordinateProgress {
            sub_id: "agent-1".to_string(),
            phase: "execution".to_string(),
            steps_completed: 3,
            steps_total: 10,
            last_action: "run tests".to_string(),
            heartbeat_epoch: 1700000000,
            outcome: None,
        };
        let display = format!("{p}");
        assert!(display.contains("agent-1"));
        assert!(display.contains("execution"));
        assert!(display.contains("3/10"));
        assert!(display.contains("run tests"));
    }

    #[test]
    fn subordinate_progress_serde_roundtrip() {
        let p = SubordinateProgress {
            sub_id: "agent-2".to_string(),
            phase: "planning".to_string(),
            steps_completed: 0,
            steps_total: 5,
            last_action: "init".to_string(),
            heartbeat_epoch: 1700000000,
            outcome: Some("success".to_string()),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: SubordinateProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn subordinate_progress_with_outcome() {
        let p = SubordinateProgress {
            sub_id: "a".to_string(),
            phase: "complete".to_string(),
            steps_completed: 5,
            steps_total: 5,
            last_action: "done".to_string(),
            heartbeat_epoch: 100,
            outcome: None,
        };
        let p2 = p.with_outcome("completed successfully");
        assert_eq!(p2.outcome, Some("completed successfully".to_string()));
        assert_eq!(p2.sub_id, "a");
    }

    #[test]
    fn subordinate_progress_new() {
        let p =
            SubordinateProgress::new("sub-1", SessionPhase::Execution, 2, 10, "compiling").unwrap();
        assert_eq!(p.sub_id, "sub-1");
        assert_eq!(p.phase, "execution");
        assert_eq!(p.steps_completed, 2);
        assert_eq!(p.steps_total, 10);
        assert!(p.heartbeat_epoch > 0);
        assert!(p.outcome.is_none());
    }

    #[test]
    fn subordinate_progress_outcome_none_by_default() {
        let p = SubordinateProgress {
            sub_id: "x".to_string(),
            phase: "intake".to_string(),
            steps_completed: 0,
            steps_total: 0,
            last_action: "".to_string(),
            heartbeat_epoch: 0,
            outcome: None,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"outcome\":null"));
    }
}
