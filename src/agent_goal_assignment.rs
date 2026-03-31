//! Goal assignment and progress tracking via the cognitive memory bridge.
//!
//! Supervisors assign goals to subordinates by writing semantic facts into
//! the hive (shared cognitive memory). Subordinates read their assigned
//! goals and report progress back through the same channel. This avoids
//! raw IPC and ensures all inter-agent communication is auditable through
//! the memory system.
//!
//! Fact conventions:
//! - Goal facts:     concept = "goal-assignment", tag = "sub:<sub_id>"
//! - Progress facts: concept = "goal-progress",   tag = "sub:<sub_id>"

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::error::SimardResult;
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::session::SessionPhase;

/// The concept used for goal assignment facts in the hive.
const GOAL_CONCEPT: &str = "goal-assignment";

/// The concept used for progress report facts in the hive.
const PROGRESS_CONCEPT: &str = "goal-progress";

/// Confidence value for goal and progress facts.
/// Set high because these are authoritative supervisor directives.
const DIRECTIVE_CONFIDENCE: f64 = 0.95;

/// Tag prefix for subordinate-scoped facts.
fn sub_tag(sub_id: &str) -> String {
    format!("sub:{sub_id}")
}

/// The source_id used for goal assignment facts.
fn goal_source_id(sub_id: &str) -> String {
    format!("supervisor:goal:{sub_id}")
}

/// The source_id used for progress report facts.
fn progress_source_id(sub_id: &str) -> String {
    format!("subordinate:progress:{sub_id}")
}

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

/// Assign a goal to a subordinate by writing a semantic fact via the bridge.
///
/// The supervisor calls this to tell a subordinate what to work on. The fact
/// is stored with concept `goal-assignment` and tagged with the subordinate's
/// ID for retrieval.
pub fn assign_goal(sub_id: &str, goal: &str, bridge: &CognitiveMemoryBridge) -> SimardResult<()> {
    bridge.store_fact(
        GOAL_CONCEPT,
        goal,
        DIRECTIVE_CONFIDENCE,
        &[sub_tag(sub_id)],
        &goal_source_id(sub_id),
    )?;
    Ok(())
}

/// Read the assigned goal for a subordinate from the hive.
///
/// The subordinate calls this on startup to discover what it should work on.
/// Returns `None` if no goal has been assigned yet. If multiple goals exist
/// (e.g. re-assignment), returns the most recently stored one.
pub fn read_assigned_goal(
    my_id: &str,
    bridge: &CognitiveMemoryBridge,
) -> SimardResult<Option<String>> {
    let facts = bridge.search_facts(&sub_tag(my_id), 10, 0.0)?;

    let goal = facts
        .into_iter()
        .rfind(|f| f.concept == GOAL_CONCEPT && f.tags.contains(&sub_tag(my_id)))
        .map(|f| f.content);

    Ok(goal)
}

/// Report progress from a subordinate back to the supervisor via the hive.
///
/// The subordinate calls this periodically (or at phase transitions) so the
/// supervisor can track liveness and completion.
pub fn report_progress(
    sub_id: &str,
    progress: &SubordinateProgress,
    bridge: &CognitiveMemoryBridge,
) -> SimardResult<()> {
    let content = serde_json::to_string(progress).map_err(|e| {
        crate::error::SimardError::BridgeCallFailed {
            bridge: "cognitive-memory".to_string(),
            method: "store_fact".to_string(),
            reason: format!("failed to serialize progress: {e}"),
        }
    })?;

    bridge.store_fact(
        PROGRESS_CONCEPT,
        &content,
        DIRECTIVE_CONFIDENCE,
        &[sub_tag(sub_id)],
        &progress_source_id(sub_id),
    )?;
    Ok(())
}

/// Poll the latest progress report for a subordinate from the hive.
///
/// The supervisor calls this to check on a subordinate's status. Returns
/// `None` if no progress has been reported yet.
pub fn poll_progress(
    sub_id: &str,
    bridge: &CognitiveMemoryBridge,
) -> SimardResult<Option<SubordinateProgress>> {
    let facts = bridge.search_facts(&sub_tag(sub_id), 10, 0.0)?;

    let progress = facts
        .into_iter()
        .rfind(|f| f.concept == PROGRESS_CONCEPT && f.tags.contains(&sub_tag(sub_id)))
        .and_then(|f| serde_json::from_str::<SubordinateProgress>(&f.content).ok());

    Ok(progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_tag_formats_correctly() {
        assert_eq!(sub_tag("agent-1"), "sub:agent-1");
    }

    #[test]
    fn progress_display_is_readable() {
        let p = SubordinateProgress {
            sub_id: "test-1".to_string(),
            phase: "execution".to_string(),
            steps_completed: 3,
            steps_total: 10,
            last_action: "ran tests".to_string(),
            heartbeat_epoch: 1000,
            outcome: None,
        };
        let display = p.to_string();
        assert!(display.contains("test-1"));
        assert!(display.contains("3/10"));
    }

    #[test]
    fn progress_serialization_round_trips() {
        let p = SubordinateProgress {
            sub_id: "test-1".to_string(),
            phase: "execution".to_string(),
            steps_completed: 5,
            steps_total: 10,
            last_action: "compiled".to_string(),
            heartbeat_epoch: 12345,
            outcome: Some("success".to_string()),
        };
        let json = serde_json::to_string(&p).expect("serialize");
        let p2: SubordinateProgress = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(p, p2);
    }

    #[test]
    fn progress_with_outcome_sets_field() {
        let p = SubordinateProgress {
            sub_id: "test-1".to_string(),
            phase: "complete".to_string(),
            steps_completed: 10,
            steps_total: 10,
            last_action: "done".to_string(),
            heartbeat_epoch: 12345,
            outcome: None,
        };
        let p2 = p.with_outcome("all tests passed");
        assert_eq!(p2.outcome, Some("all tests passed".to_string()));
    }
}
