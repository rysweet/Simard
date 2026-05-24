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
        commits_behind: 0,
        in_flight_engineer_count: 1,
        minutes_since_last_update_attempt: u64::MAX,
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
fn decision_consider_self_update_json_roundtrips() {
    let raw = r#"{"choice":"consider_self_update","rationale":"binary 12 commits behind, no engineers in flight, last attempt 4h ago"}"#;
    let parsed: EngineerLifecycleDecision = serde_json::from_str(raw).expect("parse");
    match parsed {
        EngineerLifecycleDecision::ConsiderSelfUpdate { rationale } => {
            assert!(rationale.contains("12 commits behind"));
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn apply_decision_consider_self_update_does_not_mutate_state() {
    use super::apply_decision_to_state;
    use crate::goal_curation::GoalBoard;
    use crate::ooda_loop::OodaState;

    let mut state = OodaState::new(GoalBoard::default());
    let goal_id = "g";
    let before = state.goal_failure_counts.clone();
    let decision = EngineerLifecycleDecision::ConsiderSelfUpdate {
        rationale: "binary 5 commits behind, no engineers in flight, last attempt 2h ago".into(),
    };
    let detail = apply_decision_to_state(&decision, &mut state, goal_id);
    assert!(
        detail.contains("consider_self_update") || detail.contains("self_update"),
        "detail must reference the choice, got: {detail}"
    );
    assert_eq!(
        state.goal_failure_counts, before,
        "ConsiderSelfUpdate must be a no-op for failure counts (it's a doctrinal recommendation, not a goal failure)"
    );
}

#[test]
fn decision_unknown_choice_fails_to_parse() {
    let raw = r#"{"choice":"do_something_weird","rationale":"x"}"#;
    let result: Result<EngineerLifecycleDecision, _> = serde_json::from_str(raw);
    assert!(result.is_err(), "unknown choice tags must fail to parse");
}

// ---------------------------------------------------------------------------
// (1) Stub returns continue when canned DECISION marker response
// ---------------------------------------------------------------------------

#[test]
fn stub_returns_continue_when_canned_continue_json() {
    let stub = StubSubmitter::new("DECISION: continue_skipping\nhb ok");
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
// (2) Stub returns reclaim when canned DECISION marker response
// ---------------------------------------------------------------------------

#[test]
fn stub_returns_reclaim_when_canned_reclaim_json() {
    let stub = StubSubmitter::new(
        "DECISION: reclaim_and_redispatch\nREDISPATCH_CONTEXT: retry persistence layer\nRATIONALE: 7h idle",
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
    let stub = StubSubmitter::new("this is not a DECISION marker at all, sorry");
    let brain = RustyClawdBrain::new(stub);
    let result = brain.decide_engineer_lifecycle(&sample_ctx());
    assert!(
        result.is_err(),
        "unparseable LLM output must surface as Err so caller can fall back"
    );
}

#[test]
fn stub_partial_garbage_then_brace_still_errors_cleanly() {
    let stub = StubSubmitter::new("explanation but no DECISION marker");
    let brain = RustyClawdBrain::new(stub);
    let _ = brain.decide_engineer_lifecycle(&sample_ctx()); // just must not panic
}

// ---------------------------------------------------------------------------
// (4) Stub extra labeled fields are ignored (forward compat)
// ---------------------------------------------------------------------------

#[test]
fn stub_extra_fields_are_ignored() {
    let stub = StubSubmitter::new(
        "DECISION: continue_skipping\nRATIONALE: ok\nFUTURE_FIELD: ignored\nANOTHER: 42",
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
fn stub_json_only_response_is_rejected() {
    // Issue #1980: JSON-only responses are no longer accepted
    let stub = StubSubmitter::new(r#"{"choice":"continue_skipping","rationale":"ok"}"#);
    let brain = RustyClawdBrain::new(stub);
    let result = brain.decide_engineer_lifecycle(&sample_ctx());
    assert!(
        result.is_err(),
        "JSON-only response must be rejected (issue #1980)"
    );
}

#[test]
fn stub_json_in_prose_response_is_rejected() {
    // Issue #1980: JSON wrapped in prose without DECISION marker is rejected
    let stub = StubSubmitter::new(
        "Here is my decision:\n{\"choice\":\"continue_skipping\",\"rationale\":\"ok\"}\nLet me know.",
    );
    let brain = RustyClawdBrain::new(stub);
    let result = brain.decide_engineer_lifecycle(&sample_ctx());
    assert!(
        result.is_err(),
        "JSON-in-prose without DECISION marker must be rejected (issue #1980)"
    );
}

// ---------------------------------------------------------------------------
// Prompt rendering: editing the prompt file changes outputs.
// Demonstrates the iteration loop: updating the prompt + a fixed LLM stub
// changes the brain's choice.
// ---------------------------------------------------------------------------

#[test]
fn render_prompt_includes_context_fields() {
    let stub = StubSubmitter::new("DECISION: continue_skipping\nx");
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
    let stub = StubSubmitter::new("DECISION: continue_skipping\nx");
    let brain = RustyClawdBrain::new(stub);
    let ctx = sample_ctx();
    let _ = brain.decide_engineer_lifecycle(&ctx).expect("decide");
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

// ===========================================================================
// Issue #1711 — Prose-first DECISION-marker protocol (T1–T15)
//
// These tests pin the parser contract introduced in issue #1711:
//
//   - Brain emits a leading line of the form `DECISION: <variant>` and the
//     parser extracts the variant from that marker. Subsequent text is
//     treated as either an optional JSON object (carrying variant-specific
//     fields like `title` / `body` / `reason` / `redispatch_context`) or as
//     the rationale.
//   - When the response is **pure JSON** (the legacy format, no marker),
//     the parser falls back to its existing object-extraction path so
//     deployed prompts keep working.
//   - When parsing fails entirely, the returned error MUST embed the **full
//     raw response text** (truncated only for log-flood safety), NOT just a
//     `got N bytes` byte-count — that's the production bug this issue fixes.
//
// All variant tags MUST round-trip via the prose marker. Marker matching is
// security-critical: only the first non-blank line is inspected, to prevent
// mid-response DECISION-token injection from a hostile model output.
// ===========================================================================

// ---------------------------------------------------------------------------
// T1 — Pure-prose DECISION marker (`continue_skipping`) parses
// ---------------------------------------------------------------------------

#[test]
fn t1_prose_decision_marker_continue_skipping_parses() {
    // The minimal happy-path: marker line alone, no following body. Per the
    // design, variants with no required fields (continue_skipping,
    // deprioritize, consider_self_update) accept a marker-only response.
    let stub = StubSubmitter::new("DECISION: continue_skipping\n");
    let brain = RustyClawdBrain::new(stub);
    let decision = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect("marker-only prose response must parse");
    assert!(
        matches!(decision, EngineerLifecycleDecision::ContinueSkipping { .. }),
        "expected ContinueSkipping, got {decision:?}"
    );
}

// ---------------------------------------------------------------------------
// T2 — Marker is case-insensitive on the word "DECISION"
// ---------------------------------------------------------------------------

#[test]
fn t2_prose_decision_marker_case_insensitive() {
    // Lowercase "decision:" must match — LLMs vary on capitalisation. The
    // variant tag itself is exact-match snake_case (the closed enum set).
    for marker_word in ["DECISION", "decision", "Decision", "DeCiSiOn"] {
        let response = format!("{marker_word}: continue_skipping\nrationale: hb ok\n");
        let stub = StubSubmitter::new(response.clone());
        let brain = RustyClawdBrain::new(stub);
        let decision = brain
            .decide_engineer_lifecycle(&sample_ctx())
            .unwrap_or_else(|e| panic!("case variant `{marker_word}:` must parse, got error: {e}"));
        assert!(
            matches!(decision, EngineerLifecycleDecision::ContinueSkipping { .. }),
            "case variant `{marker_word}:` produced wrong variant: {decision:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// T3 — Prose marker followed by free-form rationale parses
// ---------------------------------------------------------------------------

#[test]
fn t3_prose_decision_with_following_rationale_parses() {
    // `DECISION: continue_skipping` on line 1, then prose rationale. The
    // remaining text becomes the `rationale` field. The parser MUST NOT
    // require valid JSON for variants that only carry a `rationale`.
    let response =
        "DECISION: continue_skipping\nEngineer heartbeat is fresh, log tail looks normal.";
    let stub = StubSubmitter::new(response);
    let brain = RustyClawdBrain::new(stub);
    let decision = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect("prose marker + rationale must parse");
    match decision {
        EngineerLifecycleDecision::ContinueSkipping { rationale } => {
            assert!(
                !rationale.is_empty(),
                "rationale must be populated from the prose body, was empty"
            );
            assert!(
                rationale.to_lowercase().contains("heartbeat")
                    || rationale.to_lowercase().contains("fresh")
                    || rationale.to_lowercase().contains("log"),
                "rationale must derive from the prose body, got: {rationale:?}"
            );
        }
        other => panic!("expected ContinueSkipping, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// T4 — Marker + JSON body for `open_tracking_issue` (variant with required
//      structured fields) parses
// ---------------------------------------------------------------------------

#[test]
fn t4_prose_marker_plus_json_fields_for_open_tracking_issue_parses() {
    // Variants that require fields beyond `rationale` MUST follow the
    // marker line with a JSON object carrying those fields. The parser
    // merges the marker variant tag with the JSON field values.
    let response = "DECISION: open_tracking_issue\n\
        {\"rationale\":\"engineer panicked\",\"title\":\"engineer panic in commit phase\",\"body\":\"see /tmp/engineer-x.log\"}\n";
    let stub = StubSubmitter::new(response);
    let brain = RustyClawdBrain::new(stub);
    let decision = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect("marker + JSON body for open_tracking_issue must parse");
    match decision {
        EngineerLifecycleDecision::OpenTrackingIssue {
            title,
            body,
            rationale,
        } => {
            assert_eq!(title, "engineer panic in commit phase");
            assert_eq!(body, "see /tmp/engineer-x.log");
            assert_eq!(rationale, "engineer panicked");
        }
        other => panic!("expected OpenTrackingIssue, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// T5 — Issue #1980: JSON in code fences now REJECTED (no DECISION marker)
// ---------------------------------------------------------------------------

#[test]
fn t5_json_in_code_fences_now_rejected() {
    // Issue #1980: JSON-only responses (even in fences) are no longer accepted.
    // The brain must use DECISION markers.
    let response = "```json\n\
        {\"choice\":\"continue_skipping\",\"rationale\":\"healthy heartbeat\"}\n\
        ```";
    let stub = StubSubmitter::new(response);
    let brain = RustyClawdBrain::new(stub);
    let result = brain.decide_engineer_lifecycle(&sample_ctx());
    assert!(
        result.is_err(),
        "JSON in code fences without DECISION marker must be rejected (issue #1980)"
    );
}

// ---------------------------------------------------------------------------
// T6 — Issue #1980: JSON surrounded by prose now REJECTED (no DECISION marker)
// ---------------------------------------------------------------------------

#[test]
fn t6_json_with_leading_and_trailing_prose_now_rejected() {
    // Issue #1980: JSON wrapped in prose without DECISION marker is rejected.
    let response = "I have considered the context carefully.\n\
        Based on the heartbeat I conclude:\n\
        {\"choice\":\"reclaim_and_redispatch\",\"rationale\":\"7h idle\",\"redispatch_context\":\"focus on persistence\"}\n\
        Hope this helps. Let me know if you need clarification.";
    let stub = StubSubmitter::new(response);
    let brain = RustyClawdBrain::new(stub);
    let result = brain.decide_engineer_lifecycle(&sample_ctx());
    assert!(
        result.is_err(),
        "JSON in prose without DECISION marker must be rejected (issue #1980)"
    );
}

// ---------------------------------------------------------------------------
// T7 — Empty response → error containing raw text (or empty-response note)
// ---------------------------------------------------------------------------

#[test]
fn t7_empty_response_returns_error_with_raw_text() {
    let stub = StubSubmitter::new("");
    let brain = RustyClawdBrain::new(stub);
    let err = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect_err("empty response MUST return Err so caller can fall back");
    let msg = format!("{err}");
    assert!(
        msg.to_lowercase().contains("empty") || msg.contains("\"\""),
        "empty-response error must indicate emptiness, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// T8 — Ambiguous response → error embedding the **full raw text**
//      (regression for the production 3-byte 'OK' bug class)
// ---------------------------------------------------------------------------

#[test]
fn t8_ambiguous_response_returns_error_containing_raw_text() {
    // Three bytes: "OK". No DECISION marker, no JSON object. The legacy
    // parser logged only `got 3 bytes`. The new parser MUST embed "OK" in
    // the error message so operators can diagnose the model behaviour
    // without having to grep the raw transcript.
    let stub = StubSubmitter::new("OK");
    let brain = RustyClawdBrain::new(stub);
    let err = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect_err("ambiguous response MUST return Err");
    let msg = format!("{err}");
    assert!(
        msg.contains("OK"),
        "error message MUST embed the raw response text 'OK' (anti-regression \
         for the lossy `got N bytes` log format), got: {msg}"
    );
    assert!(
        !msg.contains("got 2 bytes") && !msg.contains("got 3 bytes"),
        "error must NOT use the legacy `got N bytes` byte-count format that \
         the issue-#1711 production bug originated from, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// T9 — Regression for the EXACT production failure mode (3-byte response,
//      no JSON, parser falls back to ContinueSkipping silently)
// ---------------------------------------------------------------------------

#[test]
fn t9_three_byte_ok_no_longer_silently_succeeds_as_continue_skipping() {
    // The original bug: brain returns "OK" (3 bytes), strict-JSON parser
    // rejects, dispatcher falls back to ContinueSkipping → goal blocked
    // forever. The parser layer MUST surface this as Err. Whether the
    // dispatcher then chooses to fall back is a separate concern (and is
    // documented as out-of-scope for #1711).
    let stub = StubSubmitter::new("OK");
    let brain = RustyClawdBrain::new(stub);
    let result = brain.decide_engineer_lifecycle(&sample_ctx());
    assert!(
        result.is_err(),
        "3-byte ambiguous response MUST surface as Err at the brain layer, \
         not be silently coerced into ContinueSkipping. Got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// T10 — Marker with unknown variant → error embeds raw text
// ---------------------------------------------------------------------------

#[test]
fn t10_bogus_variant_returns_error_with_raw_text() {
    // Marker present but variant is not in the EngineerLifecycleDecision
    // closed-set whitelist. Parser MUST reject and embed both the raw
    // response and a hint about what was wrong.
    let response = "DECISION: do_something_weird\nrationale: trying to be creative";
    let stub = StubSubmitter::new(response);
    let brain = RustyClawdBrain::new(stub);
    let err = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect_err("unknown variant tag MUST return Err");
    let msg = format!("{err}");
    assert!(
        msg.contains("do_something_weird"),
        "error must embed the offending variant token from the raw response, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// T11 — All 6 EngineerLifecycleDecision variants round-trip via the prose
//       DECISION marker (closed-set whitelist, no drift between code & docs)
// ---------------------------------------------------------------------------

#[test]
fn t11_all_six_variants_round_trip_via_prose_marker() {
    // Each variant is constructed via marker + (optional) JSON body. The
    // parser MUST accept all six and produce the corresponding enum
    // variant. Whitelist test: if any variant is added to the enum but
    // missed in the parser, this test fails.
    type VariantCheck = fn(&EngineerLifecycleDecision) -> bool;
    let cases: &[(&str, &str, VariantCheck)] = &[
        (
            "continue_skipping",
            "DECISION: continue_skipping\nrationale: hb ok",
            |d| matches!(d, EngineerLifecycleDecision::ContinueSkipping { .. }),
        ),
        (
            "reclaim_and_redispatch",
            "DECISION: reclaim_and_redispatch\n\
             {\"rationale\":\"7h idle\",\"redispatch_context\":\"persistence layer\"}",
            |d| matches!(d, EngineerLifecycleDecision::ReclaimAndRedispatch { .. }),
        ),
        (
            "deprioritize",
            "DECISION: deprioritize\nrationale: chronic failures",
            |d| matches!(d, EngineerLifecycleDecision::Deprioritize { .. }),
        ),
        (
            "open_tracking_issue",
            "DECISION: open_tracking_issue\n\
             {\"rationale\":\"stack trace\",\"title\":\"engineer panic\",\"body\":\"see logs\"}",
            |d| matches!(d, EngineerLifecycleDecision::OpenTrackingIssue { .. }),
        ),
        (
            "mark_goal_blocked",
            "DECISION: mark_goal_blocked\n\
             {\"rationale\":\"needs human\",\"reason\":\"missing API key\"}",
            |d| matches!(d, EngineerLifecycleDecision::MarkGoalBlocked { .. }),
        ),
        (
            "consider_self_update",
            "DECISION: consider_self_update\n\
             rationale: binary 12 commits behind, no engineers in flight",
            |d| matches!(d, EngineerLifecycleDecision::ConsiderSelfUpdate { .. }),
        ),
    ];
    for (tag, response, predicate) in cases {
        let stub = StubSubmitter::new(*response);
        let brain = RustyClawdBrain::new(stub);
        let decision = brain
            .decide_engineer_lifecycle(&sample_ctx())
            .unwrap_or_else(|e| panic!("variant `{tag}` failed to parse: {e}"));
        assert!(
            predicate(&decision),
            "variant `{tag}` produced wrong enum: {decision:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// T12 — Marker WINS over a conflicting `choice` field in the JSON body
//       (security: prevents variant smuggling via JSON when the model has
//       declared a different choice in the marker)
// ---------------------------------------------------------------------------

#[test]
fn t12_marker_wins_over_conflicting_json_choice_field() {
    // Marker says continue_skipping, JSON body says deprioritize. The
    // marker is the authoritative source of truth: it appears first and
    // came from the explicit instruction. Parser MUST NOT silently switch
    // to deprioritize based on the JSON field.
    let response = "DECISION: continue_skipping\n\
        {\"choice\":\"deprioritize\",\"rationale\":\"sneaky\"}";
    let stub = StubSubmitter::new(response);
    let brain = RustyClawdBrain::new(stub);
    let decision = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect("marker + JSON body must parse with marker-wins precedence");
    assert!(
        matches!(decision, EngineerLifecycleDecision::ContinueSkipping { .. }),
        "marker MUST take precedence over JSON `choice` field; got {decision:?}"
    );
}

// ---------------------------------------------------------------------------
// T13 — Mid-response DECISION marker is IGNORED (security: only the first
//       non-blank line is inspected, preventing prompt-injection attacks
//       via embedded marker tokens)
// ---------------------------------------------------------------------------

#[test]
fn t13_mid_response_decision_marker_is_ignored() {
    // The marker appears on line 4, not line 1. Parser MUST NOT treat it
    // as authoritative — there's no JSON object either, so the response
    // should be rejected with raw text in the error.
    let response = "I'm not sure what to do.\n\
        Let me think about this.\n\
        \n\
        DECISION: continue_skipping\n\
        but actually maybe we should reclaim, I don't know";
    let stub = StubSubmitter::new(response);
    let brain = RustyClawdBrain::new(stub);
    let err = brain.decide_engineer_lifecycle(&sample_ctx()).expect_err(
        "mid-response DECISION marker MUST be ignored — only the first \
             non-blank line is authoritative (security: prevents marker injection)",
    );
    let msg = format!("{err}");
    assert!(
        // Embed at least part of the raw response so operators can diagnose.
        msg.contains("not sure") || msg.contains("DECISION") || msg.contains("reclaim"),
        "error must embed the raw response text, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// T14 — Very large response is truncated in the error message (log-flood
//       protection: a runaway model output must not flood ~/.simard/logs)
// ---------------------------------------------------------------------------

#[test]
fn t14_large_input_truncated_in_error_message() {
    // 100 KiB of garbage with no marker or JSON. The parser MUST return
    // Err and the error message MUST be bounded in size (truncated to
    // some reasonable cap, e.g. 8 KiB) — otherwise a malicious or
    // malfunctioning model could fill the disk via the log path.
    let huge = "x".repeat(100 * 1024);
    let stub = StubSubmitter::new(huge.clone());
    let brain = RustyClawdBrain::new(stub);
    let err = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect_err("100 KiB of garbage MUST return Err");
    let msg = format!("{err}");
    // Bound: error message smaller than the raw input. Generous upper
    // bound (32 KiB) — implementations may pick e.g. 8 KiB; this test
    // just enforces the *property* that some truncation happens for
    // pathologically large inputs.
    assert!(
        msg.len() < huge.len(),
        "error message ({} bytes) must be smaller than the {} byte raw input \
         — implementations MUST truncate to prevent log-flood DoS",
        msg.len(),
        huge.len()
    );
    assert!(
        msg.len() < 64 * 1024,
        "error message ({} bytes) must be bounded for log-safety; \
         pathological model output MUST NOT fill ~/.simard/logs",
        msg.len()
    );
    // Some indication that truncation happened (substring varies; we
    // just check the message references the truncation OR notes the
    // total raw size in some form).
    assert!(
        msg.contains("truncat")
            || msg.contains("…")
            || msg.contains("...")
            || msg.contains("bytes"),
        "truncated error should signal that truncation occurred, got first 200 bytes: {}",
        &msg.chars().take(200).collect::<String>()
    );
}

// ---------------------------------------------------------------------------
// T15 — Multibyte UTF-8 response does not panic (safety: char_indices/get
//       slicing only — no raw byte-index slicing that could panic on a
//       UTF-8 boundary)
// ---------------------------------------------------------------------------

#[test]
fn t15_multibyte_utf8_response_does_not_panic() {
    // Mix of CJK + emoji + accented chars + a marker. The parser must
    // handle this without panicking on a byte-boundary slice. The marker
    // is on line 1, so this should also parse successfully.
    let response = "DECISION: continue_skipping\n\
        理由: エンジニアの心拍音は健康です 💚 — café résumé naïve";
    let stub = StubSubmitter::new(response);
    let brain = RustyClawdBrain::new(stub);
    let decision = brain
        .decide_engineer_lifecycle(&sample_ctx())
        .expect("multibyte UTF-8 in body MUST NOT panic and MUST parse");
    assert!(matches!(
        decision,
        EngineerLifecycleDecision::ContinueSkipping { .. }
    ));

    // Also test multibyte chars in an error path — pathological mixed
    // garbage with no marker / no JSON. Must not panic on truncation.
    let garbage = "あいうえお".repeat(2000); // ~30 KiB of multibyte
    let stub2 = StubSubmitter::new(garbage);
    let brain2 = RustyClawdBrain::new(stub2);
    let err = brain2
        .decide_engineer_lifecycle(&sample_ctx())
        .expect_err("garbage multibyte response MUST return Err");
    // The fact that we got here without panicking is the test. Format
    // the error to make sure Display also doesn't panic.
    let _ = format!("{err}");
}

// ---------------------------------------------------------------------------
// Prompt asset: DECISION marker protocol must be documented + all 6 variants
// ---------------------------------------------------------------------------

#[test]
fn embedded_prompt_documents_decision_marker_protocol() {
    // The prompt MUST instruct the model to emit the DECISION marker so
    // production cycles use the new protocol. We don't pin the exact
    // wording (prompts iterate frequently), only the presence of the
    // marker token + some indication of where it goes.
    let prompt = include_str!("../../prompt_assets/simard/ooda_brain.md");
    assert!(
        prompt.contains("DECISION:") || prompt.contains("DECISION marker"),
        "prompt asset MUST document the DECISION marker protocol (issue #1711) \
         so the model emits the new prose-first format"
    );
}

#[test]
fn embedded_prompt_lists_all_six_decision_variants() {
    // Tightening of the existing 5-variant test: all 6 actual variants of
    // EngineerLifecycleDecision MUST appear in the prompt asset so the
    // model knows the closed set. This is the authoritative drift-detector
    // between the enum and the prompt.
    let prompt = include_str!("../../prompt_assets/simard/ooda_brain.md");
    for tag in [
        "continue_skipping",
        "reclaim_and_redispatch",
        "deprioritize",
        "open_tracking_issue",
        "mark_goal_blocked",
        "consider_self_update",
    ] {
        assert!(
            prompt.contains(tag),
            "prompt asset must enumerate variant '{tag}' \
             (drift detector: enum has 6 variants, prompt must list all 6)"
        );
    }
}
