//! Tests for the Decide brain types and deterministic fallback.
//!
//! The recipe-based keyword scanner tests live in `recipe_decide.rs`.
//! This file covers: DecideJudgment serde round-trip and the
//! DeterministicFallbackDecideBrain routing table.

use super::{DecideContext, DecideJudgment, DeterministicFallbackDecideBrain, OodaDecideBrain};
use crate::ooda_loop::ActionKind;

fn ctx(goal_id: &str) -> DecideContext {
    DecideContext {
        goal_id: goal_id.to_string(),
        urgency: 0.7,
        reason: "test".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Judgment JSON round-trip
// ---------------------------------------------------------------------------

#[test]
fn judgment_advance_goal_roundtrips() {
    let raw = r#"{"choice":"advance_goal","rationale":"ordinary slug"}"#;
    let parsed: DecideJudgment = serde_json::from_str(raw).expect("parse");
    assert_eq!(parsed.action_kind(), ActionKind::AdvanceGoal);
    assert_eq!(parsed.rationale(), "ordinary slug");
}

#[test]
fn judgment_consolidate_memory_roundtrips() {
    let raw = r#"{"choice":"consolidate_memory","rationale":"reserved __memory__"}"#;
    let parsed: DecideJudgment = serde_json::from_str(raw).expect("parse");
    assert_eq!(parsed.action_kind(), ActionKind::ConsolidateMemory);
}

#[test]
fn judgment_extra_fields_are_ignored() {
    let raw = r#"{"choice":"run_improvement","rationale":"go","futurefield":42}"#;
    let parsed: DecideJudgment = serde_json::from_str(raw).expect("parse");
    assert_eq!(parsed.action_kind(), ActionKind::RunImprovement);
}

#[test]
fn judgment_safe_update_roundtrips() {
    let raw = r#"{"choice":"safe_update","rationale":"divergence >= 3, conditions met"}"#;
    let parsed: DecideJudgment = serde_json::from_str(raw).expect("parse");
    assert_eq!(parsed.action_kind(), ActionKind::SafeUpdate);
    assert_eq!(parsed.rationale(), "divergence >= 3, conditions met");
}

// ---------------------------------------------------------------------------
// DeterministicFallbackDecideBrain — preserves pre-#1458 mapping
// ---------------------------------------------------------------------------

#[test]
fn fallback_routes_memory_synthetic_to_consolidate_memory() {
    let brain = DeterministicFallbackDecideBrain;
    let j = brain.judge_decision(&ctx("__memory__")).unwrap();
    assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
}

#[test]
fn fallback_routes_improvement_synthetic_to_run_improvement() {
    let brain = DeterministicFallbackDecideBrain;
    let j = brain.judge_decision(&ctx("__improvement__")).unwrap();
    assert_eq!(j.action_kind(), ActionKind::RunImprovement);
}

#[test]
fn fallback_routes_poll_activity_synthetic_to_poll_developer_activity() {
    let brain = DeterministicFallbackDecideBrain;
    let j = brain.judge_decision(&ctx("__poll_activity__")).unwrap();
    assert_eq!(j.action_kind(), ActionKind::PollDeveloperActivity);
}

#[test]
fn fallback_routes_extract_ideas_synthetic_to_extract_ideas() {
    let brain = DeterministicFallbackDecideBrain;
    let j = brain.judge_decision(&ctx("__extract_ideas__")).unwrap();
    assert_eq!(j.action_kind(), ActionKind::ExtractIdeas);
}

#[test]
fn fallback_routes_safe_update_synthetic_to_safe_update() {
    let brain = DeterministicFallbackDecideBrain;
    let j = brain.judge_decision(&ctx("__safe_update__")).unwrap();
    assert_eq!(j.action_kind(), ActionKind::SafeUpdate);
}

#[test]
fn fallback_routes_ordinary_goal_to_advance_goal() {
    let brain = DeterministicFallbackDecideBrain;
    let j = brain.judge_decision(&ctx("ship-v1")).unwrap();
    assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
}

// ---------------------------------------------------------------------------
// decide_with_brain wire-in: brain choice flows through to PlannedAction
// ---------------------------------------------------------------------------

// (Wire-in tests live in `src/ooda_loop/decide.rs` since `decide_with_brain`
// is a private module item; co-locating tests with the function avoids
// adding a public re-export just for tests.)
