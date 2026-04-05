use super::*;
use crate::bridge::BridgeErrorPayload;
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory_bridge::CognitiveMemoryBridge;

// ── helper: mock bridges ────────────────────────────────────────────

fn empty_bridge() -> CognitiveMemoryBridge {
    let transport = InMemoryBridgeTransport::new("test-empty", |method, _params| match method {
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
