//! TDD failing tests for the per-goal-per-cycle agentic reasoner (issue #4453).
//!
//! These tests define the behavioral contract of the NEW sibling reasoner
//! BEFORE implementation. They currently fail — the referenced symbols
//! (`PerGoalAction`, `PerGoalCycleCtx`, `apply_per_goal_action_to_state`, the
//! `OodaBrain::decide_per_goal_cycle` trait method, `BrainPhase::PerGoalCycle`,
//! `BrainJudgmentRecord::from_per_goal_cycle`) do not exist yet, so the crate
//! does not compile. When the Builder phase fills in the real bodies, every
//! test in this file must pass without modification.
//!
//! Scope (design §"components", A5/A6/A7):
//!   * `PerGoalAction` — 6-variant `#[serde(tag = "choice", …)]` snake_case enum,
//!     each variant carries a mandatory `reason: String` (A5).
//!   * `apply_per_goal_action_to_state` — PURE state rail. ONLY `Reorient` and
//!     `Complete` clear `wip_refs` / roll the goal; `Continue`, `Wait`,
//!     `Investigate`, and `Spawn` MUST NOT (A6 — the root-cause fix for the
//!     70ab8541 idle→reset loop).
//!   * `DeterministicLifecycleBrain::decide_per_goal_cycle` returns `Continue`
//!     (preserves the no-LLM floor: never rolls, never reaps).
//!   * The trait method has NO default impl → an un-migrated brain cannot
//!     compile (no silent fallback, #1711).

use super::{
    BrainJudgmentRecord, BrainPhase, DeterministicLifecycleBrain, OodaBrain, PerGoalAction,
    PerGoalCycleCtx, apply_per_goal_action_to_state,
};
use crate::error::SimardResult;
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, WipRef};
use crate::ooda_loop::OodaState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn live_pr_ref() -> WipRef {
    WipRef {
        kind: "pr".to_string(),
        ref_id: "4453".to_string(),
        label: "feat: agentic per-goal-per-cycle".to_string(),
        url: Some("https://example.test/pr/4453".to_string()),
    }
}

/// A goal board with one active goal that already holds a LIVE in-flight PR
/// ref and an assigned engineer, so the mutation-table tests can prove which
/// actions preserve vs. clear the load-bearing `wip_refs`.
fn state_with_goal_holding_live_ref(goal_id: &str) -> OodaState {
    let mut goal = ActiveGoal::new(goal_id, "ship the feature", 1);
    goal.assigned_to = Some("engineer-a".to_string());
    goal.status = GoalProgress::InProgress { percent: 40 };
    goal.wip_refs = vec![live_pr_ref()];
    let mut board = GoalBoard::new();
    board.active.push(goal);
    OodaState::new(board)
}

fn sample_ctx() -> PerGoalCycleCtx {
    PerGoalCycleCtx {
        goal_id: "continuously-research-and-improve-your-own-cogn-70ab8541".into(),
        goal_description: "Continuously research and improve your own cognition. \
             STANDING PERPETUAL goal — durable improvements only"
            .into(),
        cycle_number: 12,
        ..PerGoalCycleCtx::default()
    }
}

// ---------------------------------------------------------------------------
// PerGoalAction — serde schema (A5)
// ---------------------------------------------------------------------------

#[test]
fn per_goal_action_round_trips_every_variant() {
    let variants = [
        PerGoalAction::Continue {
            reason: "engineer healthy, work in flight".into(),
        },
        PerGoalAction::Spawn {
            reason: "no live work; dispatch next source".into(),
            task_hint: "survey 3 new graph-memory papers".into(),
        },
        PerGoalAction::Reorient {
            reason: "current angle exhausted; pivot".into(),
        },
        PerGoalAction::Investigate {
            reason: "worker went quiet; inspect logs first".into(),
        },
        PerGoalAction::Wait {
            reason: "PR awaiting CI/merge".into(),
        },
        PerGoalAction::Complete {
            reason: "success criteria observed live".into(),
        },
    ];
    for action in &variants {
        let json = serde_json::to_string(action).expect("serialize");
        let back: PerGoalAction = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(action, &back, "round-trip must be lossless for {action:?}");
    }
}

#[test]
fn per_goal_action_uses_choice_tag_and_snake_case() {
    let json = serde_json::to_string(&PerGoalAction::Investigate {
        reason: "check tools".into(),
    })
    .unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        value.get("choice").and_then(|c| c.as_str()),
        Some("investigate"),
        "action must be tagged on `choice` with a snake_case label; got {json}"
    );
    assert_eq!(
        value.get("reason").and_then(|r| r.as_str()),
        Some("check tools"),
        "the mandatory reason must serialize under `reason`; got {json}"
    );
}

#[test]
fn per_goal_action_reason_is_mandatory() {
    // A choice with no `reason` field must FAIL to parse — every decision must
    // carry its reasoning (acceptance: "a recorded reason every cycle").
    let err = serde_json::from_str::<PerGoalAction>(r#"{"choice":"continue"}"#);
    assert!(
        err.is_err(),
        "a per-goal action without a reason must not parse: {err:?}"
    );
}

#[test]
fn per_goal_action_unknown_choice_is_rejected() {
    // The strict 6-variant envelope: an unknown/forged choice tag must Err, so
    // a compromised prompt cannot smuggle a novel destructive action.
    let err = serde_json::from_str::<PerGoalAction>(r#"{"choice":"reap_now","reason":"x"}"#);
    assert!(
        err.is_err(),
        "an unknown choice tag must be rejected: {err:?}"
    );
}

#[test]
fn per_goal_action_spawn_task_hint_is_optional() {
    let without: PerGoalAction =
        serde_json::from_str(r#"{"choice":"spawn","reason":"dispatch"}"#).expect("parse spawn");
    match without {
        PerGoalAction::Spawn { task_hint, .. } => {
            assert!(
                task_hint.is_empty(),
                "task_hint must default to empty when omitted, got {task_hint:?}"
            );
        }
        other => panic!("expected Spawn, got {other:?}"),
    }
    let with: PerGoalAction =
        serde_json::from_str(r#"{"choice":"spawn","reason":"dispatch","task_hint":"find source"}"#)
            .expect("parse spawn w/ hint");
    match with {
        PerGoalAction::Spawn { task_hint, .. } => assert_eq!(task_hint, "find source"),
        other => panic!("expected Spawn, got {other:?}"),
    }
}

#[test]
fn per_goal_action_ignores_unknown_extra_fields() {
    // Forward-compat: extra keys emitted by a newer prompt must be ignored, not
    // fatal, as long as the required schema is satisfied.
    let parsed: PerGoalAction = serde_json::from_str(
        r#"{"choice":"continue","reason":"healthy","confidence":0.9,"note":"extra"}"#,
    )
    .expect("extra fields must be ignored");
    assert_eq!(
        parsed,
        PerGoalAction::Continue {
            reason: "healthy".into()
        }
    );
}

#[test]
fn per_goal_action_exposes_label_and_reason_accessors() {
    let cases = [
        (
            PerGoalAction::Continue { reason: "a".into() },
            "continue",
            "a",
        ),
        (
            PerGoalAction::Spawn {
                reason: "b".into(),
                task_hint: String::new(),
            },
            "spawn",
            "b",
        ),
        (
            PerGoalAction::Reorient { reason: "c".into() },
            "reorient",
            "c",
        ),
        (
            PerGoalAction::Investigate { reason: "d".into() },
            "investigate",
            "d",
        ),
        (PerGoalAction::Wait { reason: "e".into() }, "wait", "e"),
        (
            PerGoalAction::Complete { reason: "f".into() },
            "complete",
            "f",
        ),
    ];
    for (action, label, reason) in &cases {
        assert_eq!(&action.variant_label(), label, "label for {action:?}");
        assert_eq!(&action.reason(), reason, "reason for {action:?}");
    }
}

// ---------------------------------------------------------------------------
// PerGoalAction::from_choice_fields — the SINGLE canonical closed-enum
// validation chokepoint shared by every typed-record path (the
// `simard ooda record-decision` CLI tool, which gets the fields as argv, and
// `read_verified`, which re-validates the typed record on read). These lock
// the consolidated contract so the backends can never drift apart again.
// (Group D, #4967: the old stdout-scraping `from_recipe_envelope` was removed.)
// ---------------------------------------------------------------------------

#[test]
fn from_choice_fields_builds_spawn_with_task_hint() {
    // The tool passes choice/reason/task_hint straight through as argv; the
    // chokepoint validates and builds the closed enum variant.
    let parsed = PerGoalAction::from_choice_fields("spawn", "dispatch next source", "read arxiv")
        .expect("must build spawn");
    assert_eq!(
        parsed,
        PerGoalAction::Spawn {
            reason: "dispatch next source".into(),
            task_hint: "read arxiv".into(),
        }
    );
}

#[test]
fn from_choice_fields_rejects_empty_or_whitespace_reason() {
    // Mandatory-reason invariant enforced at the single gate for EVERY backend.
    assert!(
        PerGoalAction::from_choice_fields("continue", "", "").is_none(),
        "an empty reason must not build"
    );
    assert!(
        PerGoalAction::from_choice_fields("continue", "   ", "").is_none(),
        "a whitespace-only reason must not build"
    );
}

#[test]
fn from_choice_fields_rejects_unknown_choice() {
    // A compromised prompt / hostile CLI invocation cannot smuggle a novel
    // destructive action past the closed-enum gate.
    assert!(
        PerGoalAction::from_choice_fields("reap_now", "x", "").is_none(),
        "an unknown choice must not build"
    );
}

#[test]
fn from_choice_fields_matches_choice_case_insensitively_and_trims() {
    let parsed = PerGoalAction::from_choice_fields(" Continue ", " ok ", "")
        .expect("case-insensitive trimmed choice must build");
    assert_eq!(
        parsed,
        PerGoalAction::Continue {
            reason: "ok".into()
        }
    );
}

#[test]
fn from_choice_fields_bounds_a_runaway_reason() {
    // A runaway model reason is truncated so it cannot bloat logs/records.
    let huge = "z".repeat(5_000);
    let parsed = PerGoalAction::from_choice_fields("wait", &huge, "").expect("must build");
    assert!(
        parsed.reason().chars().count() <= 501,
        "reason must be bounded, got {} chars",
        parsed.reason().chars().count()
    );
}

#[test]
fn from_choice_fields_strips_ansi_and_control_from_reason_and_task_hint() {
    // SECURITY regression (mirror of #2751): `reason`/`task_hint` are
    // model-controlled and flow verbatim to operator stderr logs and the
    // *persisted* decision record. A prompt-injected model must not be able
    // to smuggle ANSI escapes / raw C0 control bytes into those sinks to spoof
    // or hide operator log lines and audit records.
    let parsed = PerGoalAction::from_choice_fields(
        "spawn",
        "do \u{1b}[31mthing\u{1b}[0m now\u{7}\u{0}",
        "run \u{1b}[2Jclobber\u{1b}[H tests",
    )
    .expect("must build");
    let reason = parsed.reason();
    assert!(
        !reason.contains('\u{1b}'),
        "ESC (0x1b) must be stripped from reason; got {reason:?}"
    );
    assert!(
        !reason.chars().any(|c| c.is_control() && !c.is_whitespace()),
        "non-whitespace C0/DEL controls must be stripped from reason; got {reason:?}"
    );
    match parsed {
        PerGoalAction::Spawn { task_hint, .. } => {
            assert!(
                !task_hint.contains('\u{1b}'),
                "ESC (0x1b) must be stripped from task_hint; got {task_hint:?}"
            );
            assert!(
                !task_hint
                    .chars()
                    .any(|c| c.is_control() && !c.is_whitespace()),
                "non-whitespace controls must be stripped from task_hint; got {task_hint:?}"
            );
        }
        other => panic!("expected Spawn, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
//
// The crux of the 70ab8541 fix: only a DELIBERATE Reorient or Complete may
// clear the load-bearing `wip_refs`. Continue / Wait / Investigate / Spawn
// must leave them untouched, so a bursty standing goal is never self-reset.
// ---------------------------------------------------------------------------

#[test]
fn apply_continue_preserves_wip_refs_and_assignment() {
    let mut state = state_with_goal_holding_live_ref("g1");
    let _detail = apply_per_goal_action_to_state(
        &PerGoalAction::Continue {
            reason: "healthy".into(),
        },
        &mut state,
        "g1",
    );
    let g = &state.active_goals.active[0];
    assert_eq!(
        g.wip_refs.len(),
        1,
        "continue must NOT wipe wip_refs (Overseer dedup / admission / completion gate depend on them)"
    );
    assert_eq!(
        g.assigned_to.as_deref(),
        Some("engineer-a"),
        "continue must not clear the engineer assignment"
    );
    assert!(
        !matches!(g.status, GoalProgress::NotStarted),
        "continue must not roll the goal back to NotStarted"
    );
}

#[test]
fn apply_wait_and_investigate_and_spawn_preserve_wip_refs() {
    for action in [
        PerGoalAction::Wait {
            reason: "PR in CI".into(),
        },
        PerGoalAction::Investigate {
            reason: "worker quiet".into(),
        },
        PerGoalAction::Spawn {
            reason: "dispatch next".into(),
            task_hint: String::new(),
        },
    ] {
        let mut state = state_with_goal_holding_live_ref("g1");
        let _ = apply_per_goal_action_to_state(&action, &mut state, "g1");
        let g = &state.active_goals.active[0];
        assert_eq!(
            g.wip_refs.len(),
            1,
            "{action:?} must NOT wipe wip_refs — only Reorient/Complete may (A6)"
        );
        assert!(
            !matches!(g.status, GoalProgress::NotStarted),
            "{action:?} must NOT roll the goal to NotStarted"
        );
    }
}

#[test]
fn apply_investigate_never_reaps_or_reclaims() {
    // Investigate is the mandatory pre-step before any destructive action: it
    // must only be a read/look verdict, never itself tear down the worktree or
    // clear the assignment.
    let mut state = state_with_goal_holding_live_ref("g1");
    state.engineer_worktrees.clear();
    let _ = apply_per_goal_action_to_state(
        &PerGoalAction::Investigate {
            reason: "inspect logs/tools before deciding".into(),
        },
        &mut state,
        "g1",
    );
    let g = &state.active_goals.active[0];
    assert_eq!(
        g.assigned_to.as_deref(),
        Some("engineer-a"),
        "investigate must not clear the engineer assignment (no reclaim)"
    );
    assert_eq!(g.wip_refs.len(), 1, "investigate must not clear wip_refs");
}

#[test]
fn apply_reorient_rolls_and_clears_refs() {
    let mut state = state_with_goal_holding_live_ref("g1");
    let _ = apply_per_goal_action_to_state(
        &PerGoalAction::Reorient {
            reason: "angle exhausted; pivot".into(),
        },
        &mut state,
        "g1",
    );
    let g = &state.active_goals.active[0];
    assert!(
        g.wip_refs.is_empty(),
        "reorient is a DELIBERATE redirect — it clears wip_refs via roll_to_new_cycle"
    );
    assert!(
        matches!(g.status, GoalProgress::NotStarted),
        "reorient must roll the goal to NotStarted"
    );
    assert!(
        g.assigned_to.is_none(),
        "reorient must release the engineer assignment"
    );
}

#[test]
fn apply_complete_marks_completed_and_clears_refs() {
    let mut state = state_with_goal_holding_live_ref("g1");
    let _ = apply_per_goal_action_to_state(
        &PerGoalAction::Complete {
            reason: "criteria met live".into(),
        },
        &mut state,
        "g1",
    );
    let g = &state.active_goals.active[0];
    assert!(
        matches!(g.status, GoalProgress::Completed),
        "complete must set the goal status to Completed"
    );
    assert!(
        g.wip_refs.is_empty(),
        "complete may clear wip_refs (goal is done)"
    );
}

#[test]
fn apply_detail_string_names_the_chosen_action() {
    let mut state = state_with_goal_holding_live_ref("g1");
    let detail = apply_per_goal_action_to_state(
        &PerGoalAction::Continue {
            reason: "healthy engineer".into(),
        },
        &mut state,
        "g1",
    );
    assert!(
        detail.contains("continue") && detail.contains("healthy engineer"),
        "detail must name the action and carry its reason; got: {detail}"
    );
}

#[test]
fn apply_is_pure_no_panic_on_missing_goal() {
    // The rail is pure/total: applying to a goal id absent from the board must
    // not panic (best-effort, per the existing apply_decision_to_state).
    let mut state = OodaState::new(GoalBoard::new());
    let _ = apply_per_goal_action_to_state(
        &PerGoalAction::Reorient { reason: "x".into() },
        &mut state,
        "ghost",
    );
}

// ---------------------------------------------------------------------------
// Fallback brain — no-LLM floor never rolls / never reaps
// ---------------------------------------------------------------------------

#[test]
fn deterministic_brain_decides_continue_for_every_goal() {
    let brain = DeterministicLifecycleBrain;
    let action = brain
        .decide_per_goal_cycle(&sample_ctx())
        .expect("fallback must never Err");
    assert!(
        matches!(action, PerGoalAction::Continue { .. }),
        "the no-LLM fallback must always Continue (never spawn/reorient/reap), got {action:?}"
    );
}

#[test]
fn deterministic_brain_per_goal_is_stable_and_never_err() {
    let brain = DeterministicLifecycleBrain;
    let contexts = [
        PerGoalCycleCtx {
            standing_idle_signal: true,
            ..sample_ctx()
        },
        PerGoalCycleCtx {
            stale_claim_secs: Some(999_999),
            ..sample_ctx()
        },
        PerGoalCycleCtx {
            effect_board_missed: true,
            ..sample_ctx()
        },
    ];
    for ctx in &contexts {
        let r = brain.decide_per_goal_cycle(ctx);
        assert!(
            r.is_ok(),
            "fallback must never Err even on alarming signals"
        );
        assert!(
            matches!(r.unwrap(), PerGoalAction::Continue { .. }),
            "no threshold/signal may push the fallback into a destructive action"
        );
    }
}

// ---------------------------------------------------------------------------
// PerGoalCycleCtx — durable state + the 3 DEMOTED signals as inputs (A2/A4/§7)
// ---------------------------------------------------------------------------

#[test]
fn per_goal_ctx_carries_the_three_demoted_signals_as_inputs() {
    // The demoted deciders (classify_standing_idle, reap_stale_claims,
    // effect board-miss) survive ONLY as read-only inputs — never as the
    // decision. This pins their field names and that they are plain data.
    let ctx = PerGoalCycleCtx {
        standing_idle_signal: true,
        stale_claim_secs: Some(1800),
        effect_board_missed: true,
        ..sample_ctx()
    };
    assert!(ctx.standing_idle_signal);
    assert_eq!(ctx.stale_claim_secs, Some(1800));
    assert!(ctx.effect_board_missed);

    // Round-trips as data (fed to the recipe as context vars).
    let json = serde_json::to_string(&ctx).expect("serialize ctx");
    let back: PerGoalCycleCtx = serde_json::from_str(&json).expect("deserialize ctx");
    assert_eq!(back.stale_claim_secs, Some(1800));
    assert!(back.standing_idle_signal && back.effect_board_missed);
}

#[test]
fn per_goal_ctx_defaults_are_quiescent() {
    // A defaulted ctx carries NO alarming signal — best-effort gather may leave
    // any field unset and the brain reasons about partial context.
    let ctx = PerGoalCycleCtx::default();
    assert!(!ctx.standing_idle_signal);
    assert_eq!(ctx.stale_claim_secs, None);
    assert!(!ctx.effect_board_missed);
}

// ---------------------------------------------------------------------------
// BrainJudgmentRecord — every per-goal decision is recorded with its reason
// ---------------------------------------------------------------------------

#[test]
fn judgment_record_from_per_goal_cycle_captures_phase_label_and_reason() {
    let action = PerGoalAction::Spawn {
        reason: "no live work; dispatch the next source".into(),
        task_hint: "survey new papers".into(),
    };
    let record = BrainJudgmentRecord::from_per_goal_cycle("g-70ab8541", &action, false, "");
    assert_eq!(
        record.phase,
        BrainPhase::PerGoalCycle,
        "the record must be tagged with the new PerGoalCycle phase"
    );
    assert_eq!(
        record.decision, "spawn",
        "decision label must be the variant"
    );
    assert!(
        record.rationale.contains("dispatch the next source"),
        "the recorded rationale must carry the action's reason; got: {}",
        record.rationale
    );
    assert!(
        record.context_summary.contains("g-70ab8541"),
        "the context summary must identify the goal; got: {}",
        record.context_summary
    );
}

#[test]
fn brain_phase_per_goal_cycle_serialises_snake_case() {
    let json = serde_json::to_string(&BrainPhase::PerGoalCycle).unwrap();
    assert_eq!(
        json, "\"per_goal_cycle\"",
        "phase must serialise as snake_case `per_goal_cycle`; got {json}"
    );
    assert_eq!(BrainPhase::PerGoalCycle.as_str(), "per_goal_cycle");
}

// ---------------------------------------------------------------------------
// Trait shape — no default impl (compile-time no-silent-fallback, #1711)
//
// A test double MUST implement BOTH decide_engineer_lifecycle AND the new
// decide_per_goal_cycle; if the new method had a default, this scripted double
// could omit it. The fact that this compiles only once every brain implements
// the method is the guarantee that an un-migrated brain cannot ship.
// ---------------------------------------------------------------------------

struct ScriptedOnceBrain(PerGoalAction);

impl OodaBrain for ScriptedOnceBrain {
    fn decide_engineer_lifecycle(
        &self,
        _ctx: &super::EngineerLifecycleCtx,
    ) -> SimardResult<super::EngineerLifecycleDecision> {
        Ok(super::EngineerLifecycleDecision::ContinueSkipping {
            rationale: "n/a".into(),
        })
    }

    fn decide_per_goal_cycle(&self, _ctx: &PerGoalCycleCtx) -> SimardResult<PerGoalAction> {
        Ok(self.0.clone())
    }
}

#[test]
fn trait_object_dispatches_the_scripted_action() {
    let brain: Box<dyn OodaBrain> = Box::new(ScriptedOnceBrain(PerGoalAction::Investigate {
        reason: "look before you leap".into(),
    }));
    let action = brain
        .decide_per_goal_cycle(&sample_ctx())
        .expect("decision");
    assert_eq!(
        action,
        PerGoalAction::Investigate {
            reason: "look before you leap".into()
        }
    );
}
