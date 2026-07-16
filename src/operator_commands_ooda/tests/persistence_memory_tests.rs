//! Unit tests for `persist_cycle_to_memory` — the cognitive memory
//! persistence path.
//!
//! These verify the *behaviour* of the persistence path, not merely that it
//! does not panic: a recording RPC transport captures the exact
//! `memory.store_episode` calls so each test can assert that the episode was
//! stored, that its content is the canonical cycle summary, and that the
//! attached metadata reports the correct counts. Best-effort persistence must
//! still persist the right thing.

use super::*;
use crate::ooda_loop::ActionKind;
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Build an `OodaClients` whose cognitive memory records every
/// `memory.store_episode` RPC payload into the returned buffer. Only the
/// memory client is swapped; the rest of the (mock) clients are untouched.
fn recording_memories() -> (crate::ooda_loop::OodaClients, Arc<Mutex<Vec<Value>>>) {
    let episodes: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = episodes.clone();
    let transport =
        crate::rpc_transport::InMemoryRpcTransport::new("record-mem", move |method, params| {
            if method == "memory.store_episode" {
                sink.lock().expect("episode sink").push(params.clone());
                return Ok(serde_json::json!({ "id": "epi_rec" }));
            }
            Err(crate::rpc::RpcErrorPayload {
                code: -32601,
                message: format!("unexpected method for persistence path: {method}"),
            })
        });
    let mut memories = crate::ooda_actions::test_helpers::test_memories();
    memories.memory = Box::new(crate::memory_client::CognitiveMemoryClient::new(Box::new(
        transport,
    )));
    (memories, episodes)
}

/// Assert exactly one episode was stored and return its captured payload.
fn single_episode(episodes: &Arc<Mutex<Vec<Value>>>) -> Value {
    let recorded = episodes.lock().expect("episode sink");
    assert_eq!(
        recorded.len(),
        1,
        "persistence must store exactly one episode per cycle, got {}",
        recorded.len()
    );
    recorded[0].clone()
}

#[test]
fn persist_cycle_to_memory_stores_the_canonical_summary_as_a_daemon_episode() {
    let (memories, episodes) = recording_memories();
    let report = make_test_report(1);
    let expected_summary = crate::ooda_loop::summarize_cycle_report(&report);

    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    let payload = single_episode(&episodes);
    assert_eq!(
        payload["content"].as_str().expect("content"),
        expected_summary,
        "the stored episode content must be the canonical cycle summary"
    );
    assert_eq!(
        payload["source_label"].as_str().expect("source_label"),
        "ooda-daemon",
        "OODA cycle episodes are attributed to the daemon source"
    );
    let meta = &payload["metadata"];
    assert_eq!(meta["cycle_number"], 1);
    assert_eq!(meta["actions_succeeded"], 0);
    assert_eq!(meta["actions_failed"], 0);
    assert_eq!(meta["goal_count"], 0);
    assert_eq!(meta["open_issues"], 0);
}

#[test]
fn persist_cycle_to_memory_metadata_counts_goals_outcomes_and_issues() {
    let (memories, episodes) = recording_memories();
    // cycle 7: two goals, one open issue, one succeeded + one failed outcome.
    let report = make_report_with_goals_and_outcomes();

    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    let meta = single_episode(&episodes)["metadata"].clone();
    assert_eq!(meta["cycle_number"], 7);
    assert_eq!(meta["actions_succeeded"], 1);
    assert_eq!(meta["actions_failed"], 1);
    assert_eq!(meta["goal_count"], 2);
    assert_eq!(meta["open_issues"], 1);
}

#[test]
fn persist_cycle_to_memory_with_zero_outcomes_reports_zero_action_counts() {
    let (memories, episodes) = recording_memories();
    let mut report = make_test_report(5);
    report.outcomes.clear();

    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    let meta = single_episode(&episodes)["metadata"].clone();
    assert_eq!(meta["cycle_number"], 5);
    assert_eq!(meta["actions_succeeded"], 0);
    assert_eq!(meta["actions_failed"], 0);
}

#[test]
fn persist_cycle_to_memory_counts_every_failed_outcome() {
    use crate::ooda_loop::{ActionOutcome, PlannedAction};

    let (memories, episodes) = recording_memories();
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

    let meta = single_episode(&episodes)["metadata"].clone();
    assert_eq!(meta["cycle_number"], 10);
    assert_eq!(
        meta["actions_failed"], 2,
        "both failing outcomes must be counted"
    );
    assert_eq!(meta["actions_succeeded"], 0);
}

#[test]
fn persist_cycle_report_writes_a_file_and_memory_records_the_matching_episode() {
    let (memories, episodes) = recording_memories();
    let dir = tempfile::tempdir().unwrap();
    let report = make_report_with_goals_and_outcomes();
    let expected_summary = crate::ooda_loop::summarize_cycle_report(&report);

    persist_cycle_report(dir.path(), &report);
    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    // File should exist from persist_cycle_report.
    let path = dir.path().join("cycle_reports").join("cycle_7.json");
    assert!(path.exists());

    // ...and the episode persisted to memory mirrors the same cycle summary.
    let payload = single_episode(&episodes);
    assert_eq!(
        payload["content"].as_str().expect("content"),
        expected_summary
    );
    assert_eq!(payload["metadata"]["cycle_number"], 7);
}

#[test]
fn persist_cycle_to_memory_records_open_issue_count_from_the_environment() {
    use crate::ooda_loop::{EnvironmentSnapshot, GoalSnapshot, Observation};

    let (memories, episodes) = recording_memories();
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

    let meta = single_episode(&episodes)["metadata"].clone();
    assert_eq!(meta["cycle_number"], 3);
    assert_eq!(meta["goal_count"], 1);
    assert_eq!(
        meta["open_issues"], 2,
        "the two open issues in the environment snapshot must be reported"
    );
}
