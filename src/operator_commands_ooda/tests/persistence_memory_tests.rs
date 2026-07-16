//! Unit tests for `persist_cycle_to_memory` — the cognitive memory
//! persistence path.
//!
//! These tests use a *recording* cognitive-memory backend that captures every
//! `store_episode` RPC. That lets each test verify what was actually persisted
//! (the episode content and its metadata), rather than merely asserting the
//! call did not panic — the canned mock in `test_helpers` cannot observe
//! writes, so a plain "does not panic" test here would be coverage padding.

use super::*;
use crate::ooda_loop::ActionKind;

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// Build an OODA client bundle whose cognitive memory records every
/// `store_episode` call into the returned sink.
fn recording_memories() -> (crate::ooda_loop::OodaClients, Arc<Mutex<Vec<Value>>>) {
    let episodes: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&episodes);
    let transport =
        crate::rpc_transport::InMemoryRpcTransport::new("recording-mem", move |method, params| {
            match method {
                "memory.store_episode" => {
                    sink.lock().unwrap().push(params.clone());
                    Ok(json!({"id": "epi_rec"}))
                }
                "memory.get_statistics" => Ok(json!({
                    "sensory_count": 0, "working_count": 0, "episodic_count": 0,
                    "semantic_count": 0, "procedural_count": 0, "prospective_count": 0
                })),
                other => Err(crate::rpc::RpcErrorPayload {
                    code: -32601,
                    message: format!("unhandled method: {other}"),
                }),
            }
        });
    let memory: Box<dyn crate::cognitive_memory::CognitiveMemoryOps> = Box::new(
        crate::memory_client::CognitiveMemoryClient::new(Box::new(transport)),
    );
    let mut clients = crate::ooda_actions::test_helpers::test_memories();
    clients.memory = memory;
    (clients, episodes)
}

/// Assert that exactly one episode was stored and return it.
fn only_episode(episodes: &Arc<Mutex<Vec<Value>>>) -> Value {
    let recorded = episodes.lock().unwrap();
    assert_eq!(
        recorded.len(),
        1,
        "persist_cycle_to_memory must store exactly one episode per cycle"
    );
    recorded[0].clone()
}

/// Assert the stored episode's source label and metadata counters.
fn assert_episode_metadata(
    episode: &Value,
    cycle_number: u64,
    succeeded: u64,
    failed: u64,
    goal_count: u64,
    open_issues: u64,
) {
    assert_eq!(
        episode["source_label"], "ooda-daemon",
        "episodes must be attributed to the OODA daemon"
    );
    let content = episode["content"]
        .as_str()
        .expect("episode content is a string");
    assert!(!content.is_empty(), "episode summary must not be empty");
    let meta = &episode["metadata"];
    assert_eq!(meta["cycle_number"].as_u64(), Some(cycle_number));
    assert_eq!(meta["actions_succeeded"].as_u64(), Some(succeeded));
    assert_eq!(meta["actions_failed"].as_u64(), Some(failed));
    assert_eq!(meta["goal_count"].as_u64(), Some(goal_count));
    assert_eq!(meta["open_issues"].as_u64(), Some(open_issues));
}

#[test]
fn persist_cycle_to_memory_stores_one_episode_for_a_minimal_report() {
    let (memories, episodes) = recording_memories();
    let report = make_test_report(1);
    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    let episode = only_episode(&episodes);
    assert_episode_metadata(&episode, 1, 0, 0, 0, 0);
}

#[test]
fn persist_cycle_to_memory_counts_goals_and_mixed_outcomes() {
    let (memories, episodes) = recording_memories();
    let report = make_report_with_goals_and_outcomes();
    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    // Fixture: cycle 7, two goals, one open issue, one success + one failure.
    let episode = only_episode(&episodes);
    assert_episode_metadata(&episode, 7, 1, 1, 2, 1);
}

#[test]
fn persist_cycle_to_memory_reports_zero_when_no_outcomes() {
    let (memories, episodes) = recording_memories();
    let mut report = make_test_report(5);
    report.outcomes.clear();
    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    let episode = only_episode(&episodes);
    assert_episode_metadata(&episode, 5, 0, 0, 0, 0);
}

#[test]
fn persist_cycle_to_memory_counts_all_failed_outcomes_as_failures() {
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

    let episode = only_episode(&episodes);
    assert_episode_metadata(&episode, 10, 0, 2, 0, 0);
}

#[test]
fn persist_cycle_report_and_memory_together() {
    let (memories, episodes) = recording_memories();
    let dir = tempfile::tempdir().unwrap();
    let report = make_report_with_goals_and_outcomes();

    persist_cycle_report(dir.path(), &report);
    super::super::persistence::persist_cycle_to_memory(&memories, &report);

    // File should exist from persist_cycle_report ...
    let path = dir.path().join("cycle_reports").join("cycle_7.json");
    assert!(path.exists());
    // ... and the same cycle must have been mirrored into cognitive memory.
    let episode = only_episode(&episodes);
    assert_episode_metadata(&episode, 7, 1, 1, 2, 1);
}

#[test]
fn persist_cycle_to_memory_counts_open_issues_from_the_environment() {
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

    let episode = only_episode(&episodes);
    assert_episode_metadata(&episode, 3, 0, 0, 1, 2);
}
