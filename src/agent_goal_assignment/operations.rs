//! Bridge operations for assigning goals and reporting progress.

use crate::error::{SimardError, SimardResult};
use crate::memory_bridge::CognitiveMemoryBridge;

use super::types::SubordinateProgress;
use super::{DIRECTIVE_CONFIDENCE, GOAL_CONCEPT, PROGRESS_CONCEPT};
use super::{goal_source_id, progress_source_id, sub_tag};

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
