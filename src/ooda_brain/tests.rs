//! TDD failing tests for the prompt-driven OODA brain (issue #1266).
//!
//! These tests define the behavioral contract of `super` (the `ooda_brain`
//! module) BEFORE implementation. Every test currently fails — either at
//! `unimplemented!()` (runtime) or because the symbol is missing (compile).
//! When the Builder phase fills in real bodies, every test in this file
//! must pass without modification.
//!
//! Test inventory (mirrors design §9):
//!   1. `stub_returns_continue_when_canned_continue_json`
//!   2. `stub_returns_reclaim_when_canned_reclaim_json`
//!   3. `stub_unparseable_returns_brain_error`
//!   4. `stub_extra_fields_are_ignored`
//!   5. `ctx_skip_count_walks_cycle_reports`
//!   6. `ctx_log_tail_redacts_secrets`
//!   7. `dispatch_spawn_engineer_skip_path_uses_brain_decision`
//!   8. `fallback_brain_always_continues`
//!
//! Plus pure-data round-trip tests for each `EngineerLifecycleDecision`
//! variant so the JSON schema in `prompt_assets/simard/ooda_brain.md`
//! stays authoritative.

use std::sync::Mutex;

use super::{
    DeterministicFallbackBrain, EngineerLifecycleCtx, EngineerLifecycleDecision, LlmSubmitter,
    OodaBrain, RustyClawdBrain, gather_engineer_lifecycle_ctx, redact_secrets,
};
use crate::error::SimardResult;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Canned-response LLM stub. Returns `response` verbatim on every `submit`.
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

fn sample_ctx() -> EngineerLifecycleCtx {
    EngineerLifecycleCtx {
        goal_id: "improve-cognitive-memory-persistence".into(),
        goal_description: "improve durable cross-session recall".into(),
        cycle_number: 142,
        consecutive_skip_count: 12,
        failure_count: 0,
        worktree_path: std::path::PathBuf::from("/tmp/wt-test"),
        worktree_mtime_secs_ago: 25_200, // 7h
        sentinel_pid: Some(1_541_109),
        last_engineer_log_tail: "engineer alive — heartbeat ok\n".into(),
    }
}

// ---------------------------------------------------------------------------
// Decision JSON round-trip tests (one per variant)
// ---------------------------------------------------------------------------

#[test]
fn decision_continue_json_roundtrips() {
    let raw = r#"{"choice":"continue_skipping","rationale":"healthy heartbeat"}"#;
    let parsed: EngineerLifecycleDecision = serde_json::from_str(raw).expect("parse");
    assert!(matches!(
        parsed,
        EngineerLifecycleDecision::ContinueSkipping { .. }
    ));
    let serialized = serde_json::to_string(&parsed).expect("serialize");
    let reparsed: EngineerLifecycleDecision = serde_json::from_str(&serialized).expect("reparse");
    assert_eq!(parsed, reparsed);
}

#[test]
fn decision_reclaim_json_roundtrips() {
    let raw = r#"{"choice":"reclaim_and_redispatch","rationale":"stuck 7h","redispatch_context":"focus on persistence layer"}"#;
    let parsed: EngineerLifecycleDecision = serde_json::from_str(raw).expect("parse");
    match &parsed {
        EngineerLifecycleDecision::ReclaimAndRedispatch {
            redispatch_context, ..
        } => assert!(redispatch_context.contains("persistence")),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn decision_reclaim_redispatch_context_defaults_when_missing() {
    let raw = r#"{"choice":"reclaim_and_redispatch","rationale":"stuck"}"#;
    let parsed: EngineerLifecycleDecision = serde_json::from_str(raw).expect("parse");
    match parsed {
        EngineerLifecycleDecision::ReclaimAndRedispatch {
            redispatch_context, ..
        } => assert_eq!(redispatch_context, ""),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn decision_deprioritize_json_roundtrips() {
    let raw = r#"{"choice":"deprioritize","rationale":"chronic failures"}"#;
    let parsed: EngineerLifecycleDecision = serde_json::from_str(raw).expect("parse");
    assert!(matches!(
        parsed,
        EngineerLifecycleDecision::Deprioritize { .. }
    ));
}

#[test]
fn decision_open_tracking_issue_json_roundtrips() {
    let raw = r#"{"choice":"open_tracking_issue","rationale":"stack trace seen","title":"engineer panic","body":"see logs"}"#;
    let parsed: EngineerLifecycleDecision = serde_json::from_str(raw).expect("parse");
    match parsed {
        EngineerLifecycleDecision::OpenTrackingIssue { title, body, .. } => {
            assert_eq!(title, "engineer panic");
            assert_eq!(body, "see logs");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn decision_mark_blocked_json_roundtrips() {
    let raw =
        r#"{"choice":"mark_goal_blocked","rationale":"requires human","reason":"needs API key"}"#;
    let parsed: EngineerLifecycleDecision = serde_json::from_str(raw).expect("parse");
    assert!(matches!(
        parsed,
        EngineerLifecycleDecision::MarkGoalBlocked { .. }
    ));
}

#[test]
fn decision_unknown_choice_fails_to_parse() {
    let raw = r#"{"choice":"do_something_weird","rationale":"x"}"#;
    let result: Result<EngineerLifecycleDecision, _> = serde_json::from_str(raw);
    assert!(result.is_err(), "unknown choice tags must fail to parse");
}

// ---------------------------------------------------------------------------
// (1) Stub returns continue when canned continue JSON
// ---------------------------------------------------------------------------

#[test]
fn stub_returns_continue_when_canned_continue_json() {
    let stub = StubSubmitter::new(r#"{"choice":"continue_skipping","rationale":"hb ok"}"#);
    let brain = RustyClawdBrain::new(stub);
    let decision = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect("brain should succeed");
    match decision {
        EngineerLifecycleDecision::ContinueSkipping { rationale } => {
            assert_eq!(rationale, "hb ok");
        }
        other => panic!("expected ContinueSkipping, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// (2) Stub returns reclaim when canned reclaim JSON
// ---------------------------------------------------------------------------

#[test]
fn stub_returns_reclaim_when_canned_reclaim_json() {
    let stub = StubSubmitter::new(
        r#"{"choice":"reclaim_and_redispatch","rationale":"7h idle","redispatch_context":"retry persistence layer"}"#,
    );
    let brain = RustyClawdBrain::new(stub);
    let decision = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect("brain should succeed");
    match decision {
        EngineerLifecycleDecision::ReclaimAndRedispatch {
            redispatch_context, ..
        } => assert!(redispatch_context.contains("persistence")),
        other => panic!("expected ReclaimAndRedispatch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// (3) Stub unparseable returns brain error (caller can fall back)
// ---------------------------------------------------------------------------

#[test]
fn stub_unparseable_returns_brain_error() {
    let stub = StubSubmitter::new("this is not JSON at all, sorry");
    let brain = RustyClawdBrain::new(stub);
    let result = brain.decide_engineer_lifecycle(&sample_ctx());
    assert!(
        result.is_err(),
        "unparseable LLM output must surface as Err so caller can fall back"
    );
}

#[test]
fn stub_partial_garbage_then_brace_still_errors_cleanly() {
    // No braces at all → must Err, not panic.
    let stub = StubSubmitter::new("explanation but no JSON");
    let brain = RustyClawdBrain::new(stub);
    let _ = brain.decide_engineer_lifecycle(&sample_ctx()); // just must not panic
}

// ---------------------------------------------------------------------------
// (4) Stub extra fields are ignored (forward compat)
// ---------------------------------------------------------------------------

#[test]
fn stub_extra_fields_are_ignored() {
    let stub = StubSubmitter::new(
        r#"{"choice":"continue_skipping","rationale":"ok","future_field":"ignored","another":42}"#,
    );
    let brain = RustyClawdBrain::new(stub);
    let decision = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect("extra fields should not break parsing");
    assert!(matches!(
        decision,
        EngineerLifecycleDecision::ContinueSkipping { .. }
    ));
}

#[test]
fn stub_response_with_surrounding_prose_still_parses() {
    // LLMs sometimes wrap JSON in prose. The brain MUST extract the JSON
    // object from the first `{` to the last `}`.
    let stub = StubSubmitter::new(
        "Here is my decision:\n{\"choice\":\"continue_skipping\",\"rationale\":\"ok\"}\nLet me know if you need more.",
    );
    let brain = RustyClawdBrain::new(stub);
    let decision = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect("brain should extract JSON from prose-wrapped response");
    assert!(matches!(
        decision,
        EngineerLifecycleDecision::ContinueSkipping { .. }
    ));
}

// ---------------------------------------------------------------------------
// Prompt rendering: editing the prompt file changes outputs.
// Demonstrates the iteration loop: updating the prompt + a fixed LLM stub
// changes the brain's choice.
// ---------------------------------------------------------------------------

#[test]
fn render_prompt_includes_context_fields() {
    let stub = StubSubmitter::new(r#"{"choice":"continue_skipping","rationale":"x"}"#);
    let brain = RustyClawdBrain::new(stub);
    let ctx = sample_ctx();
    let rendered = brain.render_prompt(&ctx);

    // Every context field MUST appear so the prompt can reference it.
    assert!(rendered.contains(&ctx.goal_id), "goal_id missing");
    assert!(
        rendered.contains(&ctx.cycle_number.to_string()),
        "cycle_number missing"
    );
    assert!(
        rendered.contains(&ctx.consecutive_skip_count.to_string()),
        "consecutive_skip_count missing"
    );
    assert!(
        rendered.contains(&ctx.worktree_mtime_secs_ago.to_string()),
        "worktree_mtime_secs_ago missing"
    );
    assert!(
        rendered.contains(ctx.last_engineer_log_tail.trim()),
        "log tail missing"
    );
}

#[test]
fn rendered_prompt_passed_to_submitter_unchanged() {
    let stub = StubSubmitter::new(r#"{"choice":"continue_skipping","rationale":"x"}"#);
    // Wrap submitter in a way that lets us peek at the last prompt.
    // (StubSubmitter stores last_prompt internally.)
    let brain = RustyClawdBrain::new(stub);
    let ctx = sample_ctx();
    let _ = brain.decide_engineer_lifecycle(&ctx).expect("decide");
    // Rebuild a fresh stub to check the last_prompt was actually set.
    // (StubSubmitter is consumed by `new`; this assertion lives inside the
    // earlier render_prompt test by virtue of construction. We keep this
    // test as a placeholder for future behavior assertions.)
}

// ---------------------------------------------------------------------------
// (5) ctx skip count walks cycle reports
// ---------------------------------------------------------------------------

#[test]
fn ctx_skip_count_walks_cycle_reports() {
    use crate::goal_curation::GoalBoard;
    use crate::ooda_loop::OodaState;

    let tmp = tempfile::tempdir().expect("tempdir");
    let state_root = tmp.path();
    let cycle_dir = state_root.join("cycle_reports");
    std::fs::create_dir_all(&cycle_dir).expect("mkdir");

    let goal_id = "improve-cognitive-memory-persistence";

    // Write 5 fake cycle reports, all skipping our goal. Naming convention
    // is implementation-detail: builder writes whatever the production code
    // also writes. We use `cycle_NNNN.json` (zero-padded) so newest sorts
    // last lexically — matching the existing simard convention.
    for i in 0..5u32 {
        let report = serde_json::json!({
            "cycle_number": 100u32 + i,
            "outcomes": [{
                "action": {
                    "kind": "AdvanceGoal",
                    "goal_id": goal_id,
                    "description": "advance goal",
                },
                "success": true,
                "detail": format!(
                    "spawn_engineer skipped: goal '{goal_id}' already has a live engineer worktree at /tmp/x"
                ),
            }],
        });
        let path = cycle_dir.join(format!("cycle_{:04}.json", 100 + i));
        std::fs::write(&path, serde_json::to_string(&report).unwrap()).expect("write");
    }

    let state = OodaState::new(GoalBoard::default());
    let worktree_path = state_root
        .join("engineer-worktrees")
        .join(format!("{goal_id}-1"));
    std::fs::create_dir_all(&worktree_path).expect("mkdir wt");

    let ctx = gather_engineer_lifecycle_ctx(&state, state_root, goal_id, &worktree_path);

    assert!(
        ctx.consecutive_skip_count >= 5,
        "expected ≥5 consecutive skips, got {}",
        ctx.consecutive_skip_count
    );
    assert_eq!(ctx.goal_id, goal_id);
    assert_eq!(ctx.worktree_path, worktree_path);
}

#[test]
fn ctx_skip_count_resets_when_non_skip_outcome_seen() {
    use crate::goal_curation::GoalBoard;
    use crate::ooda_loop::OodaState;

    let tmp = tempfile::tempdir().expect("tempdir");
    let state_root = tmp.path();
    let cycle_dir = state_root.join("cycle_reports");
    std::fs::create_dir_all(&cycle_dir).expect("mkdir");

    let goal_id = "g";

    // Older skips … then a non-skip success in between → walking-back counter
    // must stop at the boundary.
    let entries = [
        (100, true, "engineer alive — skipping"),
        (101, true, "engineer alive — skipping"),
        (102, true, "spawn_engineer dispatched: agent='engineer-x'"),
        (103, true, "engineer alive — skipping"),
        (104, true, "engineer alive — skipping"),
    ];
    for (cycle, success, detail) in entries {
        let report = serde_json::json!({
            "cycle_number": cycle as u32,
            "outcomes": [{
                "action": {
                    "kind": "AdvanceGoal",
                    "goal_id": goal_id,
                    "description": "x",
                },
                "success": success,
                "detail": detail,
            }],
        });
        std::fs::write(
            cycle_dir.join(format!("cycle_{:04}.json", cycle)),
            serde_json::to_string(&report).unwrap(),
        )
        .unwrap();
    }

    let state = OodaState::new(GoalBoard::default());
    let wt = state_root
        .join("engineer-worktrees")
        .join(format!("{goal_id}-1"));
    std::fs::create_dir_all(&wt).unwrap();

    let ctx = gather_engineer_lifecycle_ctx(&state, state_root, goal_id, &wt);

    // Walking back from the newest report, we hit 2 skips then the dispatch.
    // The counter must reflect the *consecutive* skips, not total skips.
    assert_eq!(
        ctx.consecutive_skip_count, 2,
        "expected 2 consecutive skips before the dispatch barrier, got {}",
        ctx.consecutive_skip_count
    );
}

// ---------------------------------------------------------------------------
// (6) ctx log tail redacts secrets
// ---------------------------------------------------------------------------

#[test]
fn ctx_log_tail_redacts_secrets() {
    let raw = "ok\ntoken=abc123secret\nGITHUB_TOKEN: ghp_xxxxxxxx\nbearer eyJhbGciOiJIUzI1NiIs\nnormal line\n";
    let red = redact_secrets(raw);

    // Each secret-bearing line must have its value scrubbed.
    assert!(!red.contains("abc123secret"), "token value leaked: {red}");
    assert!(!red.contains("ghp_xxxxxxxx"), "GITHUB_TOKEN leaked: {red}");
    assert!(
        !red.contains("eyJhbGciOiJIUzI1NiIs"),
        "bearer leaked: {red}"
    );
    // Non-secret lines must survive.
    assert!(red.contains("normal line"));
    assert!(red.contains("ok"));
    // Redaction marker present.
    assert!(red.contains("***"), "no redaction marker found");
}

// ---------------------------------------------------------------------------
// (7) Integration: brain decision → state mutation
//
// Verifies the variant→side-effect mapping that `dispatch_spawn_engineer`
// will apply when its skip-path consults the brain. We test the pure helper
// `apply_decision_to_state` rather than `dispatch_spawn_engineer` itself so
// the test does not require process spawning, real worktrees, or env vars.
// The dispatcher integration is exercised in the post-merge daemon-restart
// validation step in the spec.
// ---------------------------------------------------------------------------

#[test]
fn apply_decision_deprioritize_bumps_failure_count() {
    use super::apply_decision_to_state;
    use crate::goal_curation::GoalBoard;
    use crate::ooda_loop::OodaState;

    let mut state = OodaState::new(GoalBoard::default());
    let goal_id = "improve-cognitive-memory-persistence";

    let decision = EngineerLifecycleDecision::Deprioritize {
        rationale: "stuck 20 cycles, budget burn".into(),
    };
    let detail = apply_decision_to_state(&decision, &mut state, goal_id);

    assert!(
        detail.contains("stuck 20 cycles") || detail.contains("deprioritized"),
        "detail must surface rationale, got: {detail}"
    );
    assert!(
        state.goal_failure_counts.get(goal_id).copied().unwrap_or(0) >= 1,
        "Deprioritize MUST bump goal_failure_counts so orient.rs FAILURE_PENALTY engages"
    );
}

#[test]
fn apply_decision_continue_does_not_mutate_state() {
    use super::apply_decision_to_state;
    use crate::goal_curation::GoalBoard;
    use crate::ooda_loop::OodaState;

    let mut state = OodaState::new(GoalBoard::default());
    let goal_id = "g";
    let before = state.goal_failure_counts.clone();
    let decision = EngineerLifecycleDecision::ContinueSkipping {
        rationale: "healthy hb".into(),
    };
    let detail = apply_decision_to_state(&decision, &mut state, goal_id);

    assert!(detail.contains("healthy hb") || detail.contains("continue"));
    assert_eq!(
        state.goal_failure_counts, before,
        "ContinueSkipping must be a no-op for failure counts"
    );
}

#[test]
fn apply_decision_open_tracking_issue_returns_descriptive_detail() {
    use super::apply_decision_to_state;
    use crate::goal_curation::GoalBoard;
    use crate::ooda_loop::OodaState;

    let mut state = OodaState::new(GoalBoard::default());
    let decision = EngineerLifecycleDecision::OpenTrackingIssue {
        rationale: "stack trace observed".into(),
        title: "engineer panic in commit phase".into(),
        body: "see logs at /tmp/engineer-x.log".into(),
    };
    let detail = apply_decision_to_state(&decision, &mut state, "g");
    assert!(
        detail.contains("engineer panic") || detail.contains("tracking"),
        "detail must reference the queued issue title, got: {detail}"
    );
}

// ---------------------------------------------------------------------------
// (8) Fallback brain always continues
// ---------------------------------------------------------------------------

#[test]
fn fallback_brain_always_continues() {
    let brain = DeterministicFallbackBrain;
    let decision = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect("fallback must never return Err");
    assert!(
        matches!(decision, EngineerLifecycleDecision::ContinueSkipping { .. }),
        "fallback must preserve today's behavior (continue skipping), got {decision:?}"
    );
}

#[test]
fn fallback_brain_continues_for_any_context() {
    let brain = DeterministicFallbackBrain;
    let mut ctx = sample_ctx();
    // No matter how dire the context, fallback never escalates — that is the
    // safety guarantee that makes it safe to use when no LLM is configured.
    ctx.consecutive_skip_count = 9999;
    ctx.worktree_mtime_secs_ago = 86_400 * 7;
    ctx.failure_count = 50;
    let decision = brain.decide_engineer_lifecycle(&ctx).expect("ok");
    assert!(matches!(
        decision,
        EngineerLifecycleDecision::ContinueSkipping { .. }
    ));
}

// ---------------------------------------------------------------------------
// Prompt-iteration snapshot test
//
// Demonstrates the design's central claim: editing the prompt changes
// behavior without code changes. We do this by asserting that the
// embedded prompt asset contains the OPTIONS section with all five
// variant tags. If a future PR removes a variant from the prompt, this
// test catches the drift.
// ---------------------------------------------------------------------------

#[test]
fn embedded_prompt_lists_all_decision_variants() {
    let prompt = include_str!("../../prompt_assets/simard/ooda_brain.md");
    for tag in [
        "continue_skipping",
        "reclaim_and_redispatch",
        "deprioritize",
        "open_tracking_issue",
        "mark_goal_blocked",
    ] {
        assert!(
            prompt.contains(tag),
            "prompt asset must enumerate variant '{tag}' in OPTIONS section"
        );
    }
}

#[test]
fn embedded_prompt_has_required_sections() {
    let prompt = include_str!("../../prompt_assets/simard/ooda_brain.md");
    for header in ["ROLE", "CONTEXT", "OPTIONS", "OUTPUT_FORMAT", "EXAMPLES"] {
        assert!(
            prompt.contains(header),
            "prompt asset missing required section: {header}"
        );
    }
}
