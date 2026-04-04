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

use crate::error::{SimardError, SimardResult};
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

    let fact = facts
        .into_iter()
        .rfind(|f| f.concept == PROGRESS_CONCEPT && f.tags.contains(&sub_tag(sub_id)));

    match fact {
        None => Ok(None),
        Some(f) => {
            let progress =
                serde_json::from_str::<SubordinateProgress>(&f.content).map_err(|e| {
                    SimardError::InvalidGoalRecord {
                        field: format!("progress:{sub_id}"),
                        reason: format!("failed to deserialize subordinate progress: {e}"),
                    }
                })?;
            Ok(Some(progress))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeErrorPayload;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;

    // ── helper: mock bridges ────────────────────────────────────────────

    fn empty_bridge() -> CognitiveMemoryBridge {
        let transport =
            InMemoryBridgeTransport::new("test-empty", |method, _params| match method {
                "memory.store_fact" => Ok(serde_json::json!({"id": "fact_1"})),
                "memory.search_facts" => Ok(serde_json::json!({"facts": []})),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    fn bridge_with_goal_fact() -> CognitiveMemoryBridge {
        let transport =
            InMemoryBridgeTransport::new("test-goals", |method, _params| match method {
                "memory.store_fact" => Ok(serde_json::json!({"id": "fact_1"})),
                "memory.search_facts" => Ok(serde_json::json!({
                    "facts": [{
                        "node_id": "g1",
                        "concept": "goal-assignment",
                        "content": "build feature X",
                        "confidence": 0.95,
                        "source_id": "supervisor:goal:agent-1",
                        "tags": ["sub:agent-1"]
                    }]
                })),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    fn bridge_with_progress_fact() -> CognitiveMemoryBridge {
        let progress = SubordinateProgress {
            sub_id: "agent-1".to_string(),
            phase: "execution".to_string(),
            steps_completed: 5,
            steps_total: 10,
            last_action: "testing".to_string(),
            heartbeat_epoch: 2000,
            outcome: None,
        };
        let content = serde_json::to_string(&progress).unwrap();
        let transport =
            InMemoryBridgeTransport::new("test-progress", move |method, _params| match method {
                "memory.search_facts" => Ok(serde_json::json!({
                    "facts": [{
                        "node_id": "p1",
                        "concept": "goal-progress",
                        "content": content,
                        "confidence": 0.95,
                        "source_id": "subordinate:progress:agent-1",
                        "tags": ["sub:agent-1"]
                    }]
                })),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    fn bridge_with_bad_progress() -> CognitiveMemoryBridge {
        let transport = InMemoryBridgeTransport::new("test-bad", |method, _params| match method {
            "memory.search_facts" => Ok(serde_json::json!({
                "facts": [{
                    "node_id": "p1",
                    "concept": "goal-progress",
                    "content": "not-valid-json",
                    "confidence": 0.95,
                    "source_id": "subordinate:progress:agent-1",
                    "tags": ["sub:agent-1"]
                }]
            })),
            _ => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    // ── sub_tag / source_id helpers ─────────────────────────────────────

    #[test]
    fn sub_tag_formats_correctly() {
        assert_eq!(sub_tag("agent-1"), "sub:agent-1");
    }

    #[test]
    fn sub_tag_handles_empty_id() {
        assert_eq!(sub_tag(""), "sub:");
    }

    #[test]
    fn goal_source_id_formats_correctly() {
        assert_eq!(goal_source_id("agent-1"), "supervisor:goal:agent-1");
    }

    #[test]
    fn progress_source_id_formats_correctly() {
        assert_eq!(
            progress_source_id("agent-1"),
            "subordinate:progress:agent-1"
        );
    }

    // ── SubordinateProgress Display ─────────────────────────────────────

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
    fn progress_display_includes_all_fields() {
        let p = SubordinateProgress {
            sub_id: "alpha".to_string(),
            phase: "planning".to_string(),
            steps_completed: 0,
            steps_total: 5,
            last_action: "initialized".to_string(),
            heartbeat_epoch: 999,
            outcome: None,
        };
        let display = p.to_string();
        assert!(display.contains("alpha"));
        assert!(display.contains("planning"));
        assert!(display.contains("0/5"));
        assert!(display.contains("initialized"));
    }

    // ── SubordinateProgress serialization ───────────────────────────────

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
    fn progress_serialization_with_none_outcome() {
        let p = SubordinateProgress {
            sub_id: "x".to_string(),
            phase: "intake".to_string(),
            steps_completed: 0,
            steps_total: 0,
            last_action: "none".to_string(),
            heartbeat_epoch: 0,
            outcome: None,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"outcome\":null"));
        let p2: SubordinateProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(p.outcome, p2.outcome);
    }

    #[test]
    fn progress_deserialization_rejects_invalid_json() {
        let result = serde_json::from_str::<SubordinateProgress>("not json");
        assert!(result.is_err());
    }

    #[test]
    fn progress_deserialization_rejects_missing_fields() {
        let result = serde_json::from_str::<SubordinateProgress>(r#"{"sub_id":"a","phase":"b"}"#);
        assert!(result.is_err());
    }

    // ── with_outcome ────────────────────────────────────────────────────

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

    #[test]
    fn progress_with_outcome_preserves_other_fields() {
        let p = SubordinateProgress {
            sub_id: "b".to_string(),
            phase: "execution".to_string(),
            steps_completed: 2,
            steps_total: 4,
            last_action: "running".to_string(),
            heartbeat_epoch: 500,
            outcome: None,
        };
        let p2 = p.with_outcome("done");
        assert_eq!(p2.sub_id, "b");
        assert_eq!(p2.phase, "execution");
        assert_eq!(p2.steps_completed, 2);
        assert_eq!(p2.steps_total, 4);
        assert_eq!(p2.last_action, "running");
        assert_eq!(p2.heartbeat_epoch, 500);
    }

    #[test]
    fn progress_with_outcome_overwrites_existing_outcome() {
        let p = SubordinateProgress {
            sub_id: "c".to_string(),
            phase: "complete".to_string(),
            steps_completed: 1,
            steps_total: 1,
            last_action: "done".to_string(),
            heartbeat_epoch: 100,
            outcome: Some("old".to_string()),
        };
        let p2 = p.with_outcome("new");
        assert_eq!(p2.outcome, Some("new".to_string()));
    }

    // ── assign_goal ─────────────────────────────────────────────────────

    #[test]
    fn assign_goal_succeeds_with_mock_bridge() {
        let bridge = empty_bridge();
        let result = assign_goal("agent-1", "build feature X", &bridge);
        assert!(result.is_ok());
    }

    // ── read_assigned_goal ──────────────────────────────────────────────

    #[test]
    fn read_assigned_goal_returns_none_when_empty() {
        let bridge = empty_bridge();
        let result = read_assigned_goal("agent-1", &bridge).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_assigned_goal_returns_content_when_present() {
        let bridge = bridge_with_goal_fact();
        let result = read_assigned_goal("agent-1", &bridge).unwrap();
        assert_eq!(result, Some("build feature X".to_string()));
    }

    // ── report_progress ─────────────────────────────────────────────────

    #[test]
    fn report_progress_succeeds_with_mock_bridge() {
        let bridge = empty_bridge();
        let progress = SubordinateProgress {
            sub_id: "agent-1".to_string(),
            phase: "execution".to_string(),
            steps_completed: 3,
            steps_total: 10,
            last_action: "compiled".to_string(),
            heartbeat_epoch: 1000,
            outcome: None,
        };
        let result = report_progress("agent-1", &progress, &bridge);
        assert!(result.is_ok());
    }

    // ── poll_progress ───────────────────────────────────────────────────

    #[test]
    fn poll_progress_returns_none_when_empty() {
        let bridge = empty_bridge();
        let result = poll_progress("agent-1", &bridge).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn poll_progress_returns_deserialized_progress() {
        let bridge = bridge_with_progress_fact();
        let result = poll_progress("agent-1", &bridge).unwrap();
        assert!(result.is_some());
        let p = result.unwrap();
        assert_eq!(p.sub_id, "agent-1");
        assert_eq!(p.steps_completed, 5);
        assert_eq!(p.last_action, "testing");
    }

    #[test]
    fn poll_progress_returns_error_on_bad_json() {
        let bridge = bridge_with_bad_progress();
        let result = poll_progress("agent-1", &bridge);
        assert!(result.is_err());
    }

    // ── constants ───────────────────────────────────────────────────────

    #[test]
    fn directive_confidence_is_high() {
        let c = DIRECTIVE_CONFIDENCE;
        assert!(c > 0.9, "confidence should be > 0.9, got {c}");
        assert!(c <= 1.0, "confidence should be <= 1.0, got {c}");
    }

    #[test]
    fn goal_concept_is_expected_value() {
        assert_eq!(GOAL_CONCEPT, "goal-assignment");
    }

    #[test]
    fn progress_concept_is_expected_value() {
        assert_eq!(PROGRESS_CONCEPT, "goal-progress");
    }
}
