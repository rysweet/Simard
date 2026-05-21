//! TDD tests for issue #1890 — close silent JSON-fallback path in
//! `decide_with_brain` / `orient_with_brain`.
//!
//! These tests **fail at HEAD** (Step 7 — write tests first) and pin the
//! behaviour that Step 8 must implement:
//!
//!   1. When the LLM brain returns `Err`, the per-cycle
//!      `BrainJudgmentRecord` for that goal MUST carry a
//!      `parse_failure: Some(ParseFailureRecord)` — not the historical
//!      silent `parse_failure: None` deterministic-fallback record.
//!   2. The `ParseFailureRecord` MUST be populated with the LLM error
//!      message, the raw response text (truncated, UTF-8-safe), the
//!      prompt name + version, and the `(phase, goal_id)` consecutive
//!      count.
//!   3. The cycle MUST continue — the deterministic fallback's action
//!      kind (decide) or base urgency (orient) is still emitted so the
//!      loop doesn't stall.
//!   4. Healthy `Ok(_)` returns MUST leave `parse_failure = None` (no
//!      false-positive churn on cycle reports).
//!   5. A successful parse for a `(phase, goal_id)` after a failure MUST
//!      reset the consecutive_count for that pair to 0.
//!
//! Anti-regression: pre-fix, `decide_with_brain` swallowed the brain
//! `Err(_)` into the deterministic fallback with `BrainJudgmentRecord
//! { fallback: true, rationale: "fallback-brain: prefix-routed", … }`
//! and no parse-failure breadcrumb. `improve-simard-dashboard` and
//! `fix-broken-features` ran 89 cycles at 0.00% before any operator
//! noticed — see issue #1890.

use std::collections::HashMap;

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
use crate::memory_cognitive::CognitiveStatistics;
use crate::ooda_brain::{
    BrainJudgmentRecord, BrainPhase, DecideContext, DecideJudgment, OodaDecideBrain,
    OodaOrientBrain, OrientContext, OrientJudgment,
    parse_failure::{peek_consecutive_count, reset_consecutive_count_for_tests, test_serial_guard},
};
use crate::ooda_loop::{
    EnvironmentSnapshot, Observation, OodaConfig, Priority, decide_with_brain, orient_with_brain,
};

// ---------------------------------------------------------------------------
// Test doubles (resolution A11: AlwaysErrBrain — smallest stub possible)
// ---------------------------------------------------------------------------

/// A `OodaDecideBrain` that always returns `SimardError::AdapterInvocationFailed`
/// with an embedded raw-response snippet — mirrors the production failure mode
/// from issue #1890 where the LLM returned `"OK"` and the parser produced
/// `"no JSON object; raw_response=\"OK\""`.
struct AlwaysErrDecideBrain {
    raw_response: String,
}

impl AlwaysErrDecideBrain {
    fn new(raw_response: &str) -> Self {
        Self {
            raw_response: raw_response.to_string(),
        }
    }
}

impl OodaDecideBrain for AlwaysErrDecideBrain {
    fn judge_decision(&self, _ctx: &DecideContext) -> SimardResult<DecideJudgment> {
        Err(SimardError::AdapterInvocationFailed {
            base_type: "ooda-decide-brain".to_string(),
            reason: format!(
                "decide brain response had no JSON object; raw_response={:?}",
                self.raw_response,
            ),
        })
    }
}

struct AlwaysErrOrientBrain {
    raw_response: String,
}

impl AlwaysErrOrientBrain {
    fn new(raw_response: &str) -> Self {
        Self {
            raw_response: raw_response.to_string(),
        }
    }
}

impl OodaOrientBrain for AlwaysErrOrientBrain {
    fn judge_orientation(&self, _ctx: &OrientContext) -> SimardResult<OrientJudgment> {
        Err(SimardError::AdapterInvocationFailed {
            base_type: "ooda-orient-brain".to_string(),
            reason: format!(
                "orient brain response had no JSON object; raw_response={:?}",
                self.raw_response,
            ),
        })
    }
}

/// A brain that toggles per call: first N calls fail, then succeed. Used for
/// the consecutive-counter-reset test.
struct ToggleDecideBrain {
    failures_remaining: std::sync::Mutex<u32>,
}

impl ToggleDecideBrain {
    fn new(initial_failures: u32) -> Self {
        Self {
            failures_remaining: std::sync::Mutex::new(initial_failures),
        }
    }
}

impl OodaDecideBrain for ToggleDecideBrain {
    fn judge_decision(&self, _ctx: &DecideContext) -> SimardResult<DecideJudgment> {
        let mut g = self.failures_remaining.lock().unwrap();
        if *g > 0 {
            *g -= 1;
            Err(SimardError::AdapterInvocationFailed {
                base_type: "ooda-decide-brain".to_string(),
                reason: "no JSON object; raw_response=\"OK\"".to_string(),
            })
        } else {
            Ok(DecideJudgment::AdvanceGoal {
                rationale: "llm-brain healthy after retries".to_string(),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn one_priority(goal_id: &str) -> Vec<Priority> {
    vec![Priority {
        goal_id: goal_id.to_string(),
        urgency: 0.9,
        reason: "test priority".to_string(),
    }]
}

fn observation_with_no_signals() -> Observation {
    Observation {
        goal_statuses: Vec::new(),
        gym_health: None,
        memory_stats: CognitiveStatistics::default(),
        pending_improvements: Vec::new(),
        environment: EnvironmentSnapshot::default(),
        eval_watchdog: None,
    }
}

fn board_with_one_goal(id: &str) -> GoalBoard {
    let mut board = GoalBoard::default();
    board.active.push(ActiveGoal {
        id: id.to_string(),
        description: format!("desc {id}"),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    board
}

/// Run `f` inside a fresh brain-judgment scope. Tests that rely on the
/// process-global `(phase, goal_id) -> consecutive_count` map MUST use a
/// unique `goal_id` (typically derived from the test name) AND hold the
/// `test_serial_guard()` mutex if they assert on counter values, so
/// `cargo test`'s parallel execution can't contaminate the assertion.
fn run_isolated<R>(f: impl FnOnce() -> R) -> R {
    crate::ooda_brain::with_brain_judgment_scope(|| {
        crate::ooda_brain::clear_brain_judgments();
        f()
    })
}

// ===========================================================================
// decide_with_brain — silent-fallback closure
// ===========================================================================

#[test]
fn decide_with_brain_errored_pushes_parse_failure_record() {
    // ANTI-REGRESSION (issue #1890): before this PR, decide_with_brain on
    // brain Err pushed a BrainJudgmentRecord with `parse_failure: None`
    // and the deterministic-fallback rationale — indistinguishable on
    // disk from "operator deliberately ran without an LLM brain".
    let priorities = one_priority("improve-simard-dashboard");
    let config = OodaConfig::default();
    let brain = AlwaysErrDecideBrain::new("OK");

    let records = run_isolated(|| {
        decide_with_brain(&priorities, &config, &brain).unwrap();
        crate::ooda_brain::take_brain_judgments()
    });

    assert_eq!(records.len(), 1, "exactly one judgment record per priority");
    let rec = &records[0];
    assert_eq!(rec.phase, BrainPhase::Decide);
    assert!(
        rec.parse_failure.is_some(),
        "BrainJudgmentRecord.parse_failure MUST be Some(_) on brain Err (issue #1890); \
         got None — silent fallback regressed: {:?}",
        rec,
    );
}

#[test]
fn decide_with_brain_errored_parse_failure_carries_error_and_raw_response() {
    let priorities = one_priority("g1");
    let config = OodaConfig::default();
    let brain = AlwaysErrDecideBrain::new("OK");

    let records = run_isolated(|| {
        decide_with_brain(&priorities, &config, &brain).unwrap();
        crate::ooda_brain::take_brain_judgments()
    });

    let pf = records[0]
        .parse_failure
        .as_ref()
        .expect("parse_failure must be Some on brain Err");
    assert_eq!(pf.phase, "decide");
    assert_eq!(pf.goal_id, "g1");
    assert!(
        pf.error_message.contains("ooda-decide-brain"),
        "error_message must carry the brain adapter tag: {}",
        pf.error_message,
    );
    assert!(
        pf.error_message.contains("no JSON object"),
        "error_message must carry the parser's diagnostic: {}",
        pf.error_message,
    );
    assert!(
        pf.raw_response_truncated.contains("OK"),
        "raw_response_truncated must echo the model output: {}",
        pf.raw_response_truncated,
    );
    assert_eq!(
        pf.prompt_name, "ooda_decide.md",
        "prompt_name must match the DECIDE_PROMPT_NAME constant",
    );
    assert!(
        !pf.timestamp.is_empty(),
        "timestamp must be populated for cycle-report correlation",
    );
    assert!(
        !pf.retry_attempted,
        "retry_attempted is reserved-false in this release",
    );
}

#[test]
fn decide_with_brain_errored_still_emits_planned_action() {
    // Cycle must NOT stall on a brain failure — the deterministic fallback
    // floor still produces a PlannedAction so the loop continues, while
    // the parse_failure record makes the degradation visible.
    let priorities = one_priority("ship-v1");
    let config = OodaConfig::default();
    let brain = AlwaysErrDecideBrain::new("OK");

    let actions = run_isolated(|| decide_with_brain(&priorities, &config, &brain).unwrap());

    assert_eq!(
        actions.len(),
        1,
        "deterministic fallback must still emit an action so the cycle doesn't stall",
    );
    assert_eq!(
        actions[0].kind,
        crate::ooda_loop::ActionKind::AdvanceGoal,
        "deterministic fallback for a non-synthetic goal_id routes to AdvanceGoal",
    );
}

#[test]
fn decide_with_brain_errored_record_keeps_fallback_true_for_dashboard_back_compat() {
    // Dashboards key on `fallback == true`. The deterministic floor still
    // fires, so the field stays true. The discriminator for "forced by
    // failure" vs "operator chose deterministic" is parse_failure.is_some().
    let priorities = one_priority("g1");
    let config = OodaConfig::default();
    let brain = AlwaysErrDecideBrain::new("OK");

    let records = run_isolated(|| {
        decide_with_brain(&priorities, &config, &brain).unwrap();
        crate::ooda_brain::take_brain_judgments()
    });

    assert!(
        records[0].fallback,
        "fallback must stay true for dashboard back-compat"
    );
    assert!(
        records[0].parse_failure.is_some(),
        "discriminator: parse_failure Some"
    );
}

#[test]
fn decide_with_brain_ok_path_leaves_parse_failure_none() {
    // Healthy LLM brain returns Ok — no parse failure, no schema churn.
    struct OkBrain;
    impl OodaDecideBrain for OkBrain {
        fn judge_decision(&self, _ctx: &DecideContext) -> SimardResult<DecideJudgment> {
            Ok(DecideJudgment::AdvanceGoal {
                rationale: "healthy".to_string(),
            })
        }
    }
    let priorities = one_priority("g1");
    let config = OodaConfig::default();

    let records = run_isolated(|| {
        decide_with_brain(&priorities, &config, &OkBrain).unwrap();
        crate::ooda_brain::take_brain_judgments()
    });

    assert!(
        records[0].parse_failure.is_none(),
        "healthy Ok path MUST NOT set parse_failure: false-positive churn = bug",
    );
}

#[test]
fn decide_with_brain_errored_record_serializes_parse_failure_to_json() {
    // End-to-end: the BrainJudgmentRecord MUST serialize the parse_failure
    // field so it lands in `~/.simard/cycle_reports/cycle_N.json`. This is
    // visibility channel 3 from the four-channel contract.
    let priorities = one_priority("g1");
    let config = OodaConfig::default();
    let brain = AlwaysErrDecideBrain::new("OK");

    let records = run_isolated(|| {
        decide_with_brain(&priorities, &config, &brain).unwrap();
        crate::ooda_brain::take_brain_judgments()
    });

    let json = serde_json::to_string(&records[0]).expect("BrainJudgmentRecord must serialize");
    assert!(
        json.contains("\"parse_failure\""),
        "parse_failure MUST be present in serialized cycle_report JSON: {json}",
    );
    assert!(
        json.contains("\"phase\":\"decide\""),
        "parse_failure.phase MUST serialize as \"decide\": {json}",
    );
    assert!(
        json.contains("\"goal_id\":\"g1\""),
        "parse_failure.goal_id MUST serialize: {json}",
    );
    // Round-trip — back-compat regression: older readers parsing a richer
    // record must not break.
    let back: BrainJudgmentRecord = serde_json::from_str(&json).expect("round-trip");
    assert!(back.parse_failure.is_some());
}

#[test]
fn decide_with_brain_errored_consecutive_count_increments_per_call() {
    // Resolution A6: track consecutive failures per (phase, goal_id) so
    // the gh-issue-create channel can throttle at >= 3.
    let _serial = test_serial_guard();
    let goal_id = "decide_with_brain_errored_consecutive_count-goal";
    reset_consecutive_count_for_tests(BrainPhase::Decide, goal_id);
    let priorities = vec![Priority {
        goal_id: goal_id.to_string(),
        urgency: 0.9,
        reason: "test priority".to_string(),
    }];
    let config = OodaConfig::default();
    let brain = AlwaysErrDecideBrain::new("OK");

    let count_after_three = run_isolated(|| {
        for _ in 0..3 {
            decide_with_brain(&priorities, &config, &brain).unwrap();
        }
        peek_consecutive_count(BrainPhase::Decide, goal_id)
    });
    assert_eq!(
        count_after_three, 3,
        "consecutive_count must reach 3 after three failing calls on the same goal",
    );
}

#[test]
fn decide_with_brain_consecutive_count_resets_on_next_successful_parse() {
    // Three failures, then one Ok — counter MUST reset.
    let _serial = test_serial_guard();
    let goal_id = "decide_with_brain_consecutive_count_resets-goal";
    reset_consecutive_count_for_tests(BrainPhase::Decide, goal_id);
    let priorities = vec![Priority {
        goal_id: goal_id.to_string(),
        urgency: 0.9,
        reason: "test priority".to_string(),
    }];
    let config = OodaConfig::default();
    let brain = ToggleDecideBrain::new(3);

    let after = run_isolated(|| {
        for _ in 0..4 {
            decide_with_brain(&priorities, &config, &brain).unwrap();
        }
        peek_consecutive_count(BrainPhase::Decide, goal_id)
    });
    assert_eq!(
        after, 0,
        "consecutive_count must reset to 0 on the next successful parse",
    );
}

#[test]
fn decide_with_brain_errored_continues_to_next_priority() {
    // One failing priority must NOT stop the cycle — subsequent priorities
    // still get judged. This is the per-priority degradation guarantee
    // (Pillar 11) — a single brain hiccup can't take down the whole cycle.
    struct FirstFailsThenOkBrain {
        call: std::sync::Mutex<u32>,
    }
    impl OodaDecideBrain for FirstFailsThenOkBrain {
        fn judge_decision(&self, _ctx: &DecideContext) -> SimardResult<DecideJudgment> {
            let mut c = self.call.lock().unwrap();
            *c += 1;
            if *c == 1 {
                Err(SimardError::AdapterInvocationFailed {
                    base_type: "ooda-decide-brain".to_string(),
                    reason: "no JSON object".to_string(),
                })
            } else {
                Ok(DecideJudgment::AdvanceGoal {
                    rationale: "second priority got through".to_string(),
                })
            }
        }
    }
    let priorities = vec![
        Priority {
            goal_id: "bad-goal".to_string(),
            urgency: 0.9,
            reason: "first".to_string(),
        },
        Priority {
            goal_id: "good-goal".to_string(),
            urgency: 0.8,
            reason: "second".to_string(),
        },
    ];
    let config = OodaConfig::default();
    let brain = FirstFailsThenOkBrain {
        call: std::sync::Mutex::new(0),
    };

    let (actions, records) = run_isolated(|| {
        let actions = decide_with_brain(&priorities, &config, &brain).unwrap();
        (actions, crate::ooda_brain::take_brain_judgments())
    });

    assert_eq!(actions.len(), 2, "both priorities must be processed");
    assert_eq!(records.len(), 2);
    assert!(
        records[0].parse_failure.is_some(),
        "bad-goal logged a parse_failure"
    );
    assert!(
        records[1].parse_failure.is_none(),
        "good-goal must be a clean record (no spurious parse_failure)",
    );
}

// ===========================================================================
// orient_with_brain — silent-fallback closure
// ===========================================================================

#[test]
fn orient_with_brain_errored_pushes_parse_failure_record() {
    // Same anti-regression as decide, on the orient call site.
    // `src/ooda_loop/orient.rs:98-101` historically collapsed `Err(_)` into
    // the deterministic compute() with no breadcrumb.
    let board = board_with_one_goal("fix-broken-features");
    let obs = observation_with_no_signals();
    let mut failures = HashMap::new();
    failures.insert("fix-broken-features".to_string(), 1);
    let brain = AlwaysErrOrientBrain::new("OK");

    let records = run_isolated(|| {
        orient_with_brain(&obs, &board, &failures, &brain).unwrap();
        crate::ooda_brain::take_brain_judgments()
    });

    assert_eq!(
        records.len(),
        1,
        "exactly one orient record per failing goal"
    );
    let rec = &records[0];
    assert_eq!(rec.phase, BrainPhase::Orient);
    assert!(
        rec.parse_failure.is_some(),
        "orient BrainJudgmentRecord.parse_failure MUST be Some(_) on brain Err (issue #1890)",
    );
}

#[test]
fn orient_with_brain_errored_parse_failure_carries_error_and_raw_response() {
    let board = board_with_one_goal("g1");
    let obs = observation_with_no_signals();
    let mut failures = HashMap::new();
    failures.insert("g1".to_string(), 2);
    let brain = AlwaysErrOrientBrain::new("OK");

    let records = run_isolated(|| {
        orient_with_brain(&obs, &board, &failures, &brain).unwrap();
        crate::ooda_brain::take_brain_judgments()
    });

    let pf = records[0]
        .parse_failure
        .as_ref()
        .expect("parse_failure must be Some on brain Err");
    assert_eq!(pf.phase, "orient");
    assert_eq!(pf.goal_id, "g1");
    assert!(
        pf.error_message.contains("ooda-orient-brain"),
        "error_message must carry the orient brain adapter tag: {}",
        pf.error_message,
    );
    assert!(
        pf.raw_response_truncated.contains("OK"),
        "raw_response_truncated must echo the model output: {}",
        pf.raw_response_truncated,
    );
    assert_eq!(
        pf.prompt_name, "ooda_orient.md",
        "prompt_name must match the ORIENT_PROMPT_NAME constant",
    );
}

#[test]
fn orient_with_brain_errored_still_produces_priority() {
    // The deterministic floor's demotion still applies — the goal stays on
    // the priorities list (visible to decide) and gets the legacy penalty.
    let board = board_with_one_goal("g1");
    let obs = observation_with_no_signals();
    let mut failures = HashMap::new();
    failures.insert("g1".to_string(), 1);
    let brain = AlwaysErrOrientBrain::new("OK");

    let priorities = run_isolated(|| orient_with_brain(&obs, &board, &failures, &brain).unwrap());

    let g1 = priorities
        .iter()
        .find(|p| p.goal_id == "g1")
        .expect("g1 priority must still appear (cycle does not stall)");
    // Deterministic-floor demotion: 0.8 (NotStarted) - 0.2 (1 failure) = 0.6
    assert!(
        (g1.urgency - 0.6).abs() < 1e-9,
        "deterministic-floor demotion still applies; got urgency {}",
        g1.urgency,
    );
}

#[test]
fn orient_with_brain_ok_path_leaves_parse_failure_none() {
    struct OkOrientBrain;
    impl OodaOrientBrain for OkOrientBrain {
        fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment> {
            Ok(OrientJudgment {
                adjusted_urgency: (ctx.base_urgency - 0.05).max(0.0),
                rationale: "healthy orient brain".to_string(),
                confidence: 0.9,
                demotion_applied: 0.05,
            })
        }
    }
    let board = board_with_one_goal("g1");
    let obs = observation_with_no_signals();
    let mut failures = HashMap::new();
    failures.insert("g1".to_string(), 1);

    let records = run_isolated(|| {
        orient_with_brain(&obs, &board, &failures, &OkOrientBrain).unwrap();
        crate::ooda_brain::take_brain_judgments()
    });

    assert_eq!(records.len(), 1);
    assert!(
        records[0].parse_failure.is_none(),
        "healthy Ok path MUST NOT set parse_failure",
    );
}

#[test]
fn orient_with_brain_errored_consecutive_count_increments_per_call() {
    let _serial = test_serial_guard();
    let goal_id = "orient_with_brain_errored_consecutive_count-goal";
    reset_consecutive_count_for_tests(BrainPhase::Orient, goal_id);
    let board = board_with_one_goal(goal_id);
    let obs = observation_with_no_signals();
    let mut failures = HashMap::new();
    failures.insert(goal_id.to_string(), 1);
    let brain = AlwaysErrOrientBrain::new("OK");

    let count_after_three = run_isolated(|| {
        for _ in 0..3 {
            orient_with_brain(&obs, &board, &failures, &brain).unwrap();
        }
        peek_consecutive_count(BrainPhase::Orient, goal_id)
    });
    assert_eq!(
        count_after_three, 3,
        "consecutive_count for orient must reach 3 after three failing calls",
    );
}

#[test]
fn orient_and_decide_counters_are_independent_for_same_goal() {
    // Resolution A7: (phase, goal_id) is the counter key. A decide failure
    // for goal X must not inflate the orient counter for goal X, otherwise
    // the gh-issue-create throttle would mis-fire.
    let _serial = test_serial_guard();
    let goal_id = "orient_and_decide_counters_are_independent-goal";
    reset_consecutive_count_for_tests(BrainPhase::Decide, goal_id);
    reset_consecutive_count_for_tests(BrainPhase::Orient, goal_id);
    let priorities = vec![Priority {
        goal_id: goal_id.to_string(),
        urgency: 0.9,
        reason: "test".to_string(),
    }];
    let board = board_with_one_goal(goal_id);
    let obs = observation_with_no_signals();
    let mut failures = HashMap::new();
    failures.insert(goal_id.to_string(), 1);
    let config = OodaConfig::default();
    let decide_brain = AlwaysErrDecideBrain::new("OK");
    let orient_brain = AlwaysErrOrientBrain::new("OK");

    let (decide_count, orient_count) = run_isolated(|| {
        decide_with_brain(&priorities, &config, &decide_brain).unwrap();
        decide_with_brain(&priorities, &config, &decide_brain).unwrap();
        orient_with_brain(&obs, &board, &failures, &orient_brain).unwrap();
        (
            peek_consecutive_count(BrainPhase::Decide, goal_id),
            peek_consecutive_count(BrainPhase::Orient, goal_id),
        )
    });

    assert_eq!(
        decide_count, 2,
        "decide counter must reflect its own two failures"
    );
    assert_eq!(
        orient_count, 1,
        "orient counter must reflect its own one failure"
    );
}
