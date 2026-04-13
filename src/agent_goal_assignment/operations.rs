//! Bridge operations for assigning goals and reporting progress.

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};

use super::types::SubordinateProgress;
use super::{DIRECTIVE_CONFIDENCE, GOAL_CONCEPT, PROGRESS_CONCEPT};
use super::{goal_source_id, progress_source_id, sub_tag};

/// Assign a goal to a subordinate by writing a semantic fact via the bridge.
///
/// The supervisor calls this to tell a subordinate what to work on. The fact
/// is stored with concept `goal-assignment` and tagged with the subordinate's
/// ID for retrieval.
pub fn assign_goal(sub_id: &str, goal: &str, bridge: &dyn CognitiveMemoryOps) -> SimardResult<()> {
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
    bridge: &dyn CognitiveMemoryOps,
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
    bridge: &dyn CognitiveMemoryOps,
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
    bridge: &dyn CognitiveMemoryOps,
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

    fn mock_bridge_store_ok() -> CognitiveMemoryBridge {
        let transport = InMemoryBridgeTransport::new("test-ops", |method, _params| match method {
            "memory.store_fact" => Ok(serde_json::json!({"id": "fact_1"})),
            "memory.search_facts" => Ok(serde_json::json!({"facts": []})),
            _ => Err(BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    fn mock_bridge_store_fail() -> CognitiveMemoryBridge {
        let transport = InMemoryBridgeTransport::new("test-fail", |_method, _params| {
            Err(BridgeErrorPayload {
                code: -1,
                message: "store failed".to_string(),
            })
        });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    // ── assign_goal ─────────────────────────────────────────────────

    #[test]
    fn assign_goal_empty_goal_string() {
        let bridge = mock_bridge_store_ok();
        let result = assign_goal("agent-x", "", &bridge);
        assert!(result.is_ok());
    }

    #[test]
    fn assign_goal_empty_sub_id() {
        let bridge = mock_bridge_store_ok();
        let result = assign_goal("", "some goal", &bridge);
        assert!(result.is_ok());
    }

    #[test]
    fn assign_goal_bridge_failure_propagates() {
        let bridge = mock_bridge_store_fail();
        let result = assign_goal("agent-1", "goal", &bridge);
        assert!(result.is_err());
    }

    // ── read_assigned_goal ──────────────────────────────────────────

    #[test]
    fn read_assigned_goal_empty_id() {
        let bridge = mock_bridge_store_ok();
        let result = read_assigned_goal("", &bridge).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_assigned_goal_bridge_failure_propagates() {
        let bridge = mock_bridge_store_fail();
        let result = read_assigned_goal("agent-1", &bridge);
        assert!(result.is_err());
    }

    // ── report_progress ─────────────────────────────────────────────

    #[test]
    fn report_progress_serializes_correctly() {
        let bridge = mock_bridge_store_ok();
        let progress = SubordinateProgress {
            sub_id: "a".to_string(),
            phase: "planning".to_string(),
            steps_completed: 0,
            steps_total: 0,
            last_action: "init".to_string(),
            heartbeat_epoch: 1,
            outcome: None,
        };
        let result = report_progress("a", &progress, &bridge);
        assert!(result.is_ok());
    }

    #[test]
    fn report_progress_bridge_failure_propagates() {
        let bridge = mock_bridge_store_fail();
        let progress = SubordinateProgress {
            sub_id: "a".to_string(),
            phase: "p".to_string(),
            steps_completed: 0,
            steps_total: 0,
            last_action: "x".to_string(),
            heartbeat_epoch: 0,
            outcome: None,
        };
        let result = report_progress("a", &progress, &bridge);
        assert!(result.is_err());
    }

    // ── poll_progress ───────────────────────────────────────────────

    #[test]
    fn poll_progress_empty_id() {
        let bridge = mock_bridge_store_ok();
        let result = poll_progress("", &bridge).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn poll_progress_bridge_failure_propagates() {
        let bridge = mock_bridge_store_fail();
        let result = poll_progress("agent-1", &bridge);
        assert!(result.is_err());
    }
}
