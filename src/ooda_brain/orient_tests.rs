//! Tests for the prompt-driven Orient brain — completes the prompt-driven
//! OODA round (act + decide + orient), extends PRs #1458 and #1469.

use std::sync::Mutex;

use super::{
    DeterministicFallbackOrientBrain, FAILURE_PENALTY_PER_CONSECUTIVE, LlmSubmitter,
    OodaOrientBrain, OrientContext, OrientJudgment, RustyClawdOrientBrain,
};
use crate::error::SimardResult;

// ---------------------------------------------------------------------------
// Stub LLM submitter (mirrors the pattern in tests.rs / decide_tests.rs)
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

fn ctx(failure_count: u32, base_urgency: f64) -> OrientContext {
    OrientContext {
        goal_id: "test-goal".to_string(),
        base_urgency,
        base_reason: "not yet started".to_string(),
        failure_count,
    }
}

// ---------------------------------------------------------------------------
// OrientJudgment JSON round-trip + validation
// ---------------------------------------------------------------------------

#[test]
fn judgment_roundtrips_full_payload() {
    let raw =
        r#"{"adjusted_urgency":0.4,"demotion_applied":0.4,"rationale":"1 fail","confidence":0.9}"#;
    let parsed: OrientJudgment = serde_json::from_str(raw).expect("parse");
    assert!((parsed.adjusted_urgency - 0.4).abs() < 1e-9);
    assert!((parsed.confidence - 0.9).abs() < 1e-9);
    assert_eq!(parsed.rationale, "1 fail");
}

#[test]
fn judgment_confidence_defaults_to_one_when_absent() {
    let raw = r#"{"adjusted_urgency":0.5,"rationale":"ok"}"#;
    let parsed: OrientJudgment = serde_json::from_str(raw).expect("parse");
    assert!((parsed.confidence - 1.0).abs() < 1e-9);
}

#[test]
fn judgment_extra_fields_are_ignored() {
    let raw = r#"{"adjusted_urgency":0.5,"rationale":"ok","futurefield":42}"#;
    let parsed: OrientJudgment = serde_json::from_str(raw).expect("parse");
    assert!((parsed.adjusted_urgency - 0.5).abs() < 1e-9);
}

#[test]
fn validate_rejects_escalation_above_base() {
    let j = OrientJudgment {
        adjusted_urgency: 0.9,
        rationale: "no".to_string(),
        confidence: 1.0,
        demotion_applied: 0.0,
    };
    assert!(j.validate(0.5).is_err());
}

#[test]
fn validate_accepts_equal_to_base() {
    let j = OrientJudgment {
        adjusted_urgency: 0.5,
        rationale: "no penalty".to_string(),
        confidence: 1.0,
        demotion_applied: 0.0,
    };
    assert!(j.validate(0.5).is_ok());
}

#[test]
fn validate_rejects_out_of_range() {
    let j = OrientJudgment {
        adjusted_urgency: 1.5,
        rationale: "x".to_string(),
        confidence: 1.0,
        demotion_applied: 0.0,
    };
    assert!(j.validate(2.0).is_err());
}

#[test]
fn validate_rejects_non_finite() {
    let j = OrientJudgment {
        adjusted_urgency: f64::NAN,
        rationale: "x".to_string(),
        confidence: 1.0,
        demotion_applied: 0.0,
    };
    assert!(j.validate(0.5).is_err());
}

// ---------------------------------------------------------------------------
// DeterministicFallbackOrientBrain — preserves pre-#1469 formula bit-for-bit
// ---------------------------------------------------------------------------

#[test]
fn fallback_one_failure_applies_standard_penalty() {
    let brain = DeterministicFallbackOrientBrain;
    let j = brain.judge_orientation(&ctx(1, 0.8)).unwrap();
    let expected = 0.8 - FAILURE_PENALTY_PER_CONSECUTIVE;
    assert!((j.adjusted_urgency - expected).abs() < 1e-9);
}

#[test]
fn fallback_five_failures_clamps_to_zero() {
    let brain = DeterministicFallbackOrientBrain;
    let j = brain.judge_orientation(&ctx(5, 0.8)).unwrap();
    assert!(j.adjusted_urgency.abs() < 1e-9);
}

#[test]
fn fallback_two_failures_matches_legacy_formula() {
    // Pre-#1469: urgency = (urgency - 0.2 * count).max(0.0)
    let brain = DeterministicFallbackOrientBrain;
    let j = brain.judge_orientation(&ctx(2, 0.6)).unwrap();
    let expected = (0.6_f64 - 0.4_f64).max(0.0);
    assert!((j.adjusted_urgency - expected).abs() < 1e-9);
}

#[test]
fn fallback_rationale_matches_legacy_format() {
    let brain = DeterministicFallbackOrientBrain;
    let j = brain.judge_orientation(&ctx(2, 0.6)).unwrap();
    // Legacy format from src/ooda_loop/orient.rs: "{count} consecutive failure(s) → urgency {urgency:.2} − {penalty:.2}"
    assert_eq!(
        j.rationale,
        "2 consecutive failure(s) → urgency 0.60 − 0.40"
    );
}

#[test]
fn fallback_judgment_passes_validate() {
    let context = ctx(3, 0.9);
    let j = DeterministicFallbackOrientBrain::compute(&context);
    j.validate(context.base_urgency).expect("must validate");
}

// ---------------------------------------------------------------------------
// RustyClawdOrientBrain — round-trip via stub submitter
// ---------------------------------------------------------------------------

#[test]
fn rustyclawd_brain_parses_canned_response() {
    let stub = StubSubmitter::new(
        r#"{"adjusted_urgency":0.5,"demotion_applied":0.3,"rationale":"transient","confidence":0.7}"#,
    );
    let brain = RustyClawdOrientBrain::new(stub);
    let j = brain.judge_orientation(&ctx(1, 0.8)).unwrap();
    assert!((j.adjusted_urgency - 0.5).abs() < 1e-9);
    assert_eq!(j.rationale, "transient");
}

#[test]
fn rustyclawd_brain_parses_response_wrapped_in_prose() {
    let stub = StubSubmitter::new(
        "Here is my answer:\n```json\n{\"adjusted_urgency\":0.0,\"rationale\":\"chronic\",\"confidence\":0.95}\n```\nThanks.",
    );
    let brain = RustyClawdOrientBrain::new(stub);
    let j = brain.judge_orientation(&ctx(5, 0.8)).unwrap();
    assert!(j.adjusted_urgency.abs() < 1e-9);
}

#[test]
fn rustyclawd_brain_unparseable_returns_error() {
    let stub = StubSubmitter::new("totally not json");
    let brain = RustyClawdOrientBrain::new(stub);
    let err = brain.judge_orientation(&ctx(1, 0.8)).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("ooda-orient-brain"), "got: {msg}");
}

#[test]
fn rustyclawd_brain_rejects_escalation() {
    let stub =
        StubSubmitter::new(r#"{"adjusted_urgency":0.95,"rationale":"escalate","confidence":1.0}"#);
    let brain = RustyClawdOrientBrain::new(stub);
    // base_urgency=0.5 → 0.95 is escalation → must error so caller falls back.
    let err = brain.judge_orientation(&ctx(1, 0.5)).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("ooda-orient-brain"), "got: {msg}");
}

#[test]
fn rustyclawd_brain_renders_prompt_with_context_fields() {
    let stub = StubSubmitter::new(r#"{"adjusted_urgency":0.0,"rationale":"x","confidence":1.0}"#);
    let brain = RustyClawdOrientBrain::new(stub);
    let prompt = brain.render_prompt(&OrientContext {
        goal_id: "marker-goal-id".to_string(),
        base_urgency: 0.42,
        base_reason: "marker-reason".to_string(),
        failure_count: 7,
    });
    assert!(prompt.contains("marker-goal-id"));
    assert!(prompt.contains("marker-reason"));
    assert!(prompt.contains("0.420"));
    assert!(prompt.contains("\"failure_count\": 7"));
}

// ---------------------------------------------------------------------------
// Trait object compiles for both impls (compile-time check via dyn dispatch)
// ---------------------------------------------------------------------------

#[test]
fn trait_object_compiles_for_both_impls() {
    let stub = StubSubmitter::new(r#"{"adjusted_urgency":0.5,"rationale":"x","confidence":1.0}"#);
    let brains: Vec<Box<dyn OodaOrientBrain>> = vec![
        Box::new(DeterministicFallbackOrientBrain),
        Box::new(RustyClawdOrientBrain::new(stub)),
    ];
    for b in &brains {
        let _ = b.judge_orientation(&ctx(1, 0.8)).unwrap();
    }
}
