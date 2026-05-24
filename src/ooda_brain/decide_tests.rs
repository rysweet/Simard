//! Tests for the prompt-driven Decide brain (extends PR #1458 pattern).

use std::sync::Mutex;

use super::{
    DecideContext, DecideJudgment, DeterministicFallbackDecideBrain, LlmSubmitter, OodaDecideBrain,
    RustyClawdDecideBrain,
};
use crate::error::SimardResult;
use crate::ooda_loop::ActionKind;

// ---------------------------------------------------------------------------
// Stub LLM submitter (mirrors the pattern in tests.rs)
// ---------------------------------------------------------------------------

struct StubSubmitter {
    response: String,
    last_prompt: Mutex<Option<String>>,
}

impl StubSubmitter {
    fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            last_prompt: Mutex::new(None),
        }
    }
}

impl LlmSubmitter for StubSubmitter {
    fn submit(&self, rendered_prompt: &str) -> SimardResult<String> {
        *self.last_prompt.lock().unwrap() = Some(rendered_prompt.to_string());
        Ok(self.response.clone())
    }
}

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
// RustyClawdDecideBrain — round-trip via stub submitter
// ---------------------------------------------------------------------------

#[test]
fn rustyclawd_brain_parses_canned_advance_goal_response() {
    let stub = StubSubmitter::new("DECISION: advance_goal\nstub says go");
    let brain = RustyClawdDecideBrain::new(stub);
    let j = brain.judge_decision(&ctx("ship-v1")).unwrap();
    assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
    assert!(j.rationale().contains("stub says go"));
}

#[test]
fn rustyclawd_brain_parses_response_with_marker() {
    let stub = StubSubmitter::new("DECISION: consolidate_memory\nreserved");
    let brain = RustyClawdDecideBrain::new(stub);
    let j = brain.judge_decision(&ctx("__memory__")).unwrap();
    assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
}

#[test]
fn rustyclawd_brain_rejects_json_only_response() {
    // JSON without DECISION marker is now rejected (issue #1980)
    let stub = StubSubmitter::new(r#"{"choice":"advance_goal","rationale":"ok"}"#);
    let brain = RustyClawdDecideBrain::new(stub);
    let err = brain.judge_decision(&ctx("ship-v1")).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("ooda-decide-brain"), "got: {msg}");
}

#[test]
fn rustyclawd_brain_unparseable_returns_error() {
    let stub = StubSubmitter::new("totally not json");
    let brain = RustyClawdDecideBrain::new(stub);
    let err = brain.judge_decision(&ctx("ship-v1")).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("ooda-decide-brain"), "got: {msg}");
}

// ---------------------------------------------------------------------------
// Issue #1711 — Error messages must embed the **raw response text**, not a
// lossy `got N bytes` byte-count. Same anti-regression as rustyclawd.rs.
// ---------------------------------------------------------------------------

#[test]
fn issue_1711_unparseable_error_embeds_raw_response_text() {
    let stub = StubSubmitter::new("OK");
    let brain = RustyClawdDecideBrain::new(stub);
    let err = brain.judge_decision(&ctx("ship-v1")).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("OK"),
        "decide-brain error MUST embed the raw response text 'OK' (issue #1711 \
         anti-regression for lossy `got N bytes` log format), got: {msg}"
    );
    assert!(
        !msg.contains("got 2 bytes") && !msg.contains("got 3 bytes"),
        "decide-brain error must NOT use the legacy `got N bytes` byte-count \
         format that issue #1711 eliminated, got: {msg}"
    );
}

#[test]
fn issue_1711_empty_response_error_does_not_silently_say_zero_bytes() {
    let stub = StubSubmitter::new("");
    let brain = RustyClawdDecideBrain::new(stub);
    let err = brain.judge_decision(&ctx("ship-v1")).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.to_lowercase().contains("empty") || msg.contains("\"\""),
        "decide-brain empty-response error must indicate emptiness, got: {msg}"
    );
}

#[test]
fn rustyclawd_brain_renders_prompt_with_context_fields() {
    let stub = StubSubmitter::new("DECISION: advance_goal\nok");
    let brain = RustyClawdDecideBrain::new(stub);
    let prompt = brain.render_prompt(&DecideContext {
        goal_id: "marker-goal-id".to_string(),
        urgency: 0.42,
        reason: "marker-reason".to_string(),
    });
    assert!(prompt.contains("marker-goal-id"));
    assert!(prompt.contains("marker-reason"));
    assert!(prompt.contains("0.420"));
}

// ---------------------------------------------------------------------------
// decide_with_brain wire-in: brain choice flows through to PlannedAction
// ---------------------------------------------------------------------------

// (Wire-in tests live in `src/ooda_loop/decide.rs` since `decide_with_brain`
// is a private module item; co-locating tests with the function avoids
// adding a public re-export just for tests.)
