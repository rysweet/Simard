//! Unit tests for `persist_cycle_to_memory` — the cognitive memory
//! persistence path.
//!
//! These tests verify *what* the persistence path writes: the episode content
//! and the derived metadata (cycle number, succeeded/failed action counts, goal
//! count, open-issue count). Earlier versions only checked that the call did not
//! panic, which passed even if the wrong episode — or none at all — was stored.

use super::*;
use crate::ooda_loop::{ActionKind, summarize_cycle_report};
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Assert exactly one episode was stored and return its captured RPC params.
fn only_episode(episodes: &Arc<Mutex<Vec<Value>>>) -> Value {
    let guard = episodes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        guard.len(),
        1,
        "persist_cycle_to_memory must store exactly one episode per cycle"
    );
    guard[0].clone()
}

/// Assert the stable envelope every persisted cycle episode must carry: the
/// content is the canonical cycle summary and it is attributed to the daemon.
fn assert_envelope(params: &Value, report: &crate::ooda_loop::CycleReport) {
    assert_eq!(
        params["content"].as_str().expect("content is a string"),
        summarize_cycle_report(report),
        "episode content must be the canonical cycle summary"
    );
    assert_eq!(
        params["source_label"].as_str().expect("source label"),
        "ooda-daemon",
        "cycle episodes must be attributed to the ooda daemon"
    );
}

fn assert_metadata(params: &Value, succeeded: u64, failed: u64, goals: u64, open_issues: u64) {
    let meta = &params["metadata"];
    assert_eq!(meta["actions_succeeded"], succeeded, "actions_succeeded");
    assert_eq!(meta["actions_failed"], failed, "actions_failed");
    assert_eq!(meta["goal_count"], goals, "goal_count");
    assert_eq!(meta["open_issues"], open_issues, "open_issues");
}

#[test]
fn persist_cycle_to_memory_stores_summary_for_minimal_report() {
    let (memories, episodes) = crate::ooda_actions::test_helpers::capturing_memories();
    let report = make_test_report(1);

    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    let params = only_episode(&episodes);
    assert_envelope(&params, &report);
    assert_eq!(params["metadata"]["cycle_number"], 1);
    // Minimal report: no outcomes, no goals, no open issues.
    assert_metadata(&params, 0, 0, 0, 0);
}

#[test]
fn persist_cycle_to_memory_counts_mixed_outcomes_and_goals() {
    let (memories, episodes) = crate::ooda_actions::test_helpers::capturing_memories();
    let report = make_report_with_goals_and_outcomes();

    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    let params = only_episode(&episodes);
    assert_envelope(&params, &report);
    assert_eq!(params["metadata"]["cycle_number"], 7);
    // Two goals, one succeeded + one failed outcome, one open issue.
    assert_metadata(&params, 1, 1, 2, 1);
}

#[test]
fn persist_cycle_to_memory_reports_zero_when_no_outcomes() {
    let (memories, episodes) = crate::ooda_actions::test_helpers::capturing_memories();
    let mut report = make_test_report(5);
    report.outcomes.clear();

    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    let params = only_episode(&episodes);
    assert_envelope(&params, &report);
    assert_eq!(params["metadata"]["cycle_number"], 5);
    assert_metadata(&params, 0, 0, 0, 0);
}

#[test]
fn persist_cycle_to_memory_counts_all_failed_outcomes() {
    use crate::ooda_loop::{ActionOutcome, PlannedAction};

    let (memories, episodes) = crate::ooda_actions::test_helpers::capturing_memories();
    let mut report = make_test_report(10);
    report.outcomes = vec![
        ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::AdvanceGoal,
                goal_id: Some("g1".to_string()),
                description: "try advance".to_string(),
            },
            success: false,
            detail: "blocked".to_string(),
        },
        ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::RunGymEval,
                goal_id: None,
                description: "eval".to_string(),
            },
            success: false,
            detail: "timeout".to_string(),
        },
    ];

    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    let params = only_episode(&episodes);
    assert_envelope(&params, &report);
    assert_eq!(params["metadata"]["cycle_number"], 10);
    // Both outcomes failed: zero succeeded, two failed.
    assert_metadata(&params, 0, 2, 0, 0);
}

#[test]
fn persist_cycle_report_and_memory_together() {
    let (memories, episodes) = crate::ooda_actions::test_helpers::capturing_memories();
    let dir = tempfile::tempdir().unwrap();
    let report = make_report_with_goals_and_outcomes();

    persist_cycle_report(dir.path(), &report);
    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    // File should exist from persist_cycle_report.
    let path = dir.path().join("cycle_reports").join("cycle_7.json");
    assert!(path.exists());
    // And the episode must have been stored with the matching cycle number.
    let params = only_episode(&episodes);
    assert_eq!(params["metadata"]["cycle_number"], 7);
}

#[test]
fn persist_cycle_to_memory_counts_open_issues() {
    use crate::ooda_loop::{EnvironmentSnapshot, GoalSnapshot, Observation};

    let (memories, episodes) = crate::ooda_actions::test_helpers::capturing_memories();
    let mut report = make_test_report(3);
    report.observation = Observation {
        goal_statuses: vec![GoalSnapshot {
            id: "g1".to_string(),
            description: "fix bug".to_string(),
            progress: GoalProgress::InProgress { percent: 50 },
        }],
        gym_health: None,
        memory_stats: CognitiveStatistics::default(),
        pending_improvements: vec![],
        environment: EnvironmentSnapshot {
            git_status: String::new(),
            open_issues: vec!["issue-1".to_string(), "issue-2".to_string()],
            recent_commits: vec!["abc123".to_string()],
        },
        eval_watchdog: None,
    };

    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    let params = only_episode(&episodes);
    assert_envelope(&params, &report);
    assert_eq!(params["metadata"]["cycle_number"], 3);
    // One goal, two open issues, no outcomes.
    assert_metadata(&params, 0, 0, 1, 2);
}
