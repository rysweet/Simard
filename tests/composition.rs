//! Integration tests for Phase 5: Agent Composition & Subordinate Spawning.

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Value, json};

use simard::{
    AgentRole, BridgeErrorPayload, CognitiveMemoryBridge, HeartbeatStatus, InMemoryBridgeTransport,
    SubordinateConfig, SubordinateHandle, SubordinateIdentity, SubordinateProgress, assign_goal,
    check_heartbeat, compose_identity, identity_for_role, kill_subordinate, max_subordinate_depth,
    poll_progress, read_assigned_goal, report_progress, role_for_objective, spawn_subordinate,
};

struct StoredFact {
    node_id: String,
    concept: String,
    content: String,
    confidence: f64,
    source_id: String,
    tags: Vec<String>,
}

fn mock_hive_bridge() -> CognitiveMemoryBridge {
    let store: &'static Mutex<Vec<StoredFact>> = Box::leak(Box::new(Mutex::new(Vec::new())));
    let transport = InMemoryBridgeTransport::new("test-hive", move |method, params| match method {
        "memory.store_fact" => {
            let mut facts = store.lock().unwrap();
            let id = format!("fact-{}", facts.len() + 1);
            facts.push(StoredFact {
                node_id: id.clone(),
                concept: params["concept"].as_str().unwrap_or("").to_string(),
                content: params["content"].as_str().unwrap_or("").to_string(),
                confidence: params["confidence"].as_f64().unwrap_or(0.0),
                source_id: params["source_id"].as_str().unwrap_or("").to_string(),
                tags: params["tags"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
            });
            Ok(json!({"id": id}))
        }
        "memory.search_facts" => {
            let query = params["query"].as_str().unwrap_or("");
            let limit = params["limit"].as_u64().unwrap_or(10) as usize;
            let facts = store.lock().unwrap();
            let matching: Vec<Value> = facts
                .iter()
                .filter(|f| {
                    f.concept.contains(query)
                        || f.content.contains(query)
                        || f.tags.iter().any(|t| t.contains(query))
                })
                .take(limit)
                .map(|f| {
                    json!({
                        "node_id": f.node_id, "concept": f.concept,
                        "content": f.content, "confidence": f.confidence,
                        "source_id": f.source_id, "tags": f.tags,
                    })
                })
                .collect();
            Ok(json!({"facts": matching}))
        }
        _ => Err(BridgeErrorPayload {
            code: -32601,
            message: format!("method not found: {method}"),
        }),
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

fn test_config(name: &str, goal: &str) -> SubordinateConfig {
    SubordinateConfig {
        agent_name: name.to_string(),
        goal: goal.to_string(),
        role: AgentRole::Engineer,
        worktree_path: PathBuf::from("/tmp/test-worktree"),
        current_depth: 0,
    }
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Create a test handle without spawning a real process.
fn test_handle(name: &str, goal: &str) -> SubordinateHandle {
    SubordinateHandle {
        pid: 0,
        agent_name: name.to_string(),
        goal: goal.to_string(),
        worktree_path: PathBuf::from("/tmp/test-worktree"),
        spawn_time: now_epoch(),
        retry_count: 0,
        killed: false,
    }
}

fn progress(sub_id: &str, epoch: u64, outcome: Option<&str>) -> SubordinateProgress {
    SubordinateProgress {
        sub_id: sub_id.to_string(),
        phase: "execution".to_string(),
        steps_completed: 5,
        steps_total: 10,
        last_action: "working".to_string(),
        heartbeat_epoch: epoch,
        outcome: outcome.map(String::from),
    }
}

// Subordinate lifecycle

#[test]
fn spawn_mock_subordinate_and_verify_handle() {
    let handle = test_handle("sub-1", "build it");
    assert_eq!(handle.agent_name, "sub-1");
    assert!(!handle.killed);
    assert!(handle.spawn_time > 0);
}

#[test]
fn heartbeat_dead_when_no_progress_reported() {
    let handle = test_handle("sub-hb-1", "goal");
    let status = check_heartbeat(&handle, &mock_hive_bridge()).expect("check");
    assert_eq!(status, HeartbeatStatus::Dead);
}

#[test]
fn heartbeat_alive_after_progress_reported() {
    let handle = test_handle("sub-hb-2", "goal");
    let bridge = mock_hive_bridge();
    report_progress(
        "sub-hb-2",
        &progress("sub-hb-2", now_epoch(), None),
        &bridge,
    )
    .expect("rpt");
    match check_heartbeat(&handle, &bridge).expect("check") {
        HeartbeatStatus::Alive { phase, .. } => assert_eq!(phase, "execution"),
        other => panic!("expected Alive, got {other}"),
    }
}

#[test]
fn heartbeat_stale_with_old_epoch() {
    let handle = test_handle("sub-hb-3", "goal");
    let bridge = mock_hive_bridge();
    report_progress("sub-hb-3", &progress("sub-hb-3", 1000, None), &bridge).expect("rpt");
    match check_heartbeat(&handle, &bridge).expect("check") {
        HeartbeatStatus::Stale { seconds_since } => assert!(seconds_since > 100),
        other => panic!("expected Stale, got {other}"),
    }
}

#[test]
fn heartbeat_dead_after_kill() {
    let mut handle = test_handle("sub-hb-4", "goal");
    let bridge = mock_hive_bridge();
    report_progress(
        "sub-hb-4",
        &progress("sub-hb-4", now_epoch(), None),
        &bridge,
    )
    .expect("rpt");
    kill_subordinate(&mut handle).expect("kill");
    assert_eq!(
        check_heartbeat(&handle, &bridge).expect("check"),
        HeartbeatStatus::Dead
    );
}

// Goal assignment

#[test]
fn assign_goal_and_read_it_back() {
    let bridge = mock_hive_bridge();
    assign_goal("sub-g1", "implement parser", &bridge).expect("assign");
    let goal = read_assigned_goal("sub-g1", &bridge)
        .expect("read")
        .expect("present");
    assert_eq!(goal, "implement parser");
}

#[test]
fn read_goal_returns_none_when_unassigned() {
    assert!(
        read_assigned_goal("nonexistent", &mock_hive_bridge())
            .expect("read")
            .is_none()
    );
}

#[test]
fn assign_goal_overwrites_with_latest() {
    let bridge = mock_hive_bridge();
    assign_goal("sub-g2", "first", &bridge).expect("a1");
    assign_goal("sub-g2", "second", &bridge).expect("a2");
    let goal = read_assigned_goal("sub-g2", &bridge)
        .expect("read")
        .expect("present");
    assert_eq!(goal, "second");
}

// Progress reporting

#[test]
fn report_and_poll_progress() {
    let bridge = mock_hive_bridge();
    let p = progress("sub-p1", 99999, None);
    report_progress("sub-p1", &p, &bridge).expect("report");
    let polled = poll_progress("sub-p1", &bridge)
        .expect("poll")
        .expect("present");
    assert_eq!(polled.sub_id, "sub-p1");
    assert_eq!(polled.steps_completed, 5);
}

#[test]
fn poll_progress_returns_none_when_unreported() {
    assert!(
        poll_progress("nonexistent", &mock_hive_bridge())
            .expect("poll")
            .is_none()
    );
}

#[test]
fn progress_with_outcome_round_trips() {
    let bridge = mock_hive_bridge();
    let p = progress("sub-p2", 88888, Some("all tests passed"));
    report_progress("sub-p2", &p, &bridge).expect("report");
    let polled = poll_progress("sub-p2", &bridge)
        .expect("poll")
        .expect("present");
    assert_eq!(polled.outcome, Some("all tests passed".to_string()));
}

// Composite identity

#[test]
fn composite_identity_with_multiple_subordinates() {
    let primary = identity_for_role(AgentRole::Engineer).expect("primary");
    let mut s1 = identity_for_role(AgentRole::Reviewer).expect("s1");
    s1.name = "sub-reviewer-1".to_string();
    let mut s2 = identity_for_role(AgentRole::Engineer).expect("s2");
    s2.name = "sub-engineer-1".to_string();

    let composite = compose_identity(
        primary.clone(),
        vec![
            SubordinateIdentity {
                manifest: s1,
                role: AgentRole::Reviewer,
                max_depth: 1,
            },
            SubordinateIdentity {
                manifest: s2,
                role: AgentRole::Engineer,
                max_depth: 2,
            },
        ],
    )
    .expect("compose");

    assert_eq!(composite.primary.name, primary.name);
    assert_eq!(composite.subordinates.len(), 2);
}

#[test]
fn composite_identity_rejects_depth_exceeding_limit() {
    let primary = identity_for_role(AgentRole::Engineer).expect("primary");
    let mut sub = identity_for_role(AgentRole::Reviewer).expect("sub");
    sub.name = "sub-deep".to_string();
    let depth = max_subordinate_depth();
    let err = compose_identity(
        primary,
        vec![SubordinateIdentity {
            manifest: sub,
            role: AgentRole::Reviewer,
            max_depth: depth + 1,
        }],
    )
    .expect_err("should reject");
    assert!(err.to_string().contains("max_depth"));
}

// Role catalog

#[test]
fn role_for_objective_integration() {
    assert_eq!(role_for_objective("review the code"), AgentRole::Reviewer);
    assert_eq!(role_for_objective("run benchmark"), AgentRole::GymRunner);
    assert_eq!(
        role_for_objective("facilitate meeting"),
        AgentRole::Facilitator
    );
    assert_eq!(role_for_objective("add feature"), AgentRole::Engineer);
}

#[test]
fn identity_for_all_roles_produces_valid_manifests() {
    for role in [
        AgentRole::Engineer,
        AgentRole::Reviewer,
        AgentRole::GymRunner,
        AgentRole::Facilitator,
    ] {
        let m = identity_for_role(role).unwrap_or_else(|e| panic!("{role} failed: {e}"));
        assert_eq!(m.default_mode, role.operating_mode());
    }
}

// Feral / edge cases

#[test]
fn spawn_rejects_depth_at_limit() {
    let config = SubordinateConfig {
        agent_name: "sub-deep".to_string(),
        goal: "goal".to_string(),
        role: AgentRole::Engineer,
        worktree_path: PathBuf::from("/tmp/deep"),
        current_depth: max_subordinate_depth(),
    };
    let err = spawn_subordinate(&config).expect_err("reject");
    assert!(err.to_string().contains("depth"));
}

#[test]
fn retry_policy_enforced() {
    let mut handle = test_handle("sub-retry", "flaky");
    assert!(handle.can_retry());
    handle.record_retry();
    assert!(handle.can_retry());
    handle.record_retry();
    assert!(!handle.can_retry());
}

#[test]
fn kill_idempotency_check() {
    let mut handle = test_handle("sub-kill", "doomed");
    kill_subordinate(&mut handle).expect("first kill");
    let err = kill_subordinate(&mut handle).expect_err("second kill");
    assert!(err.to_string().contains("already killed"));
}

#[test]
fn goal_isolation_between_subordinates() {
    let bridge = mock_hive_bridge();
    assign_goal("sub-a", "goal A", &bridge).expect("a");
    assign_goal("sub-b", "goal B", &bridge).expect("b");
    assert_eq!(
        read_assigned_goal("sub-a", &bridge).unwrap().unwrap(),
        "goal A"
    );
    assert_eq!(
        read_assigned_goal("sub-b", &bridge).unwrap().unwrap(),
        "goal B"
    );
}

#[test]
fn progress_isolation_between_subordinates() {
    let bridge = mock_hive_bridge();
    report_progress("sub-iso-a", &progress("sub-iso-a", 11111, None), &bridge).expect("a");
    report_progress("sub-iso-b", &progress("sub-iso-b", 22222, None), &bridge).expect("b");
    let a = poll_progress("sub-iso-a", &bridge).unwrap().unwrap();
    let b = poll_progress("sub-iso-b", &bridge).unwrap().unwrap();
    assert_eq!(a.sub_id, "sub-iso-a");
    assert_eq!(b.sub_id, "sub-iso-b");
}
