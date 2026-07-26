//! Integration tests for issue #1025 — graceful OODA completion + bounded
//! reflection safeguard, exercised through the crate's **public** pure API
//! (`simard::ooda_loop::completion`).
//!
//! These lock the terminal-completion contract at the public boundary the
//! daemon (`run_ooda_daemon`) consumes:
//!
//!   * terminal path  — a gate-verified goal yields `GracefulComplete`;
//!   * running path    — a criteria-unmet goal keeps `Continue`;
//!   * bound path      — a stuck non-perpetual goal yields `BoundExceeded`;
//!   * perpetual        — standing goals are exempt from the bound;
//!   * board idle       — `goals_all_achieved` only when every goal is verified;
//!   * perpetual-default — with default bounds nothing is spin-capped.
//!
//! The daemon consumes these decisions in `run_ooda_daemon`
//! (`operator_commands_ooda::daemon`): a gate-verified goal is auto-completed
//! and logged `ACHIEVED (gate-verified)`, the opt-in `SIMARD_OODA_STOP_WHEN_ACHIEVED`
//! idles a drained board, and `SIMARD_OODA_MAX_REFLECTION_CYCLES` yields a stuck
//! non-perpetual goal with a recorded blocker. The daemon-side glue
//! (`reflection_bound_yields`, `should_graceful_idle_stop`) is unit-tested in
//! that module; this suite locks the pure decision contract it builds on.

use std::collections::BTreeMap;

use simard::goal_curation::{
    ActiveGoal, CompletionEvidence, CompletionVerdict, GoalBoard, MissingEvidence,
};
use simard::ooda_loop::completion::{LoopDecision, ReflectionBounds, evaluate, goals_all_achieved};

fn complete() -> CompletionVerdict {
    CompletionVerdict::Complete(CompletionEvidence {
        pr_merged: true,
        issue_closed: true,
        self_affecting: false,
        deployed: true,
    })
}

fn blocked() -> CompletionVerdict {
    CompletionVerdict::Blocked {
        evidence: CompletionEvidence {
            pr_merged: false,
            issue_closed: false,
            self_affecting: false,
            deployed: true,
        },
        missing: vec![MissingEvidence::PrNotMerged],
    }
}

fn goal(id: &str) -> ActiveGoal {
    ActiveGoal::new(id, format!("deliver {id}"), 100)
}

fn standing_goal(id: &str) -> ActiveGoal {
    ActiveGoal::new(id, format!("standing {id}"), 100).mark_standing()
}

fn bounds(max: u32, stop_when_idle: bool) -> ReflectionBounds {
    ReflectionBounds {
        max_reflection_cycles: max,
        stop_when_idle,
    }
}

#[test]
fn terminal_path_green_pr_completes_gracefully() {
    // Deliverable PR merged/green + gate verified => loop exits.
    let g = goal("terminal");
    assert_eq!(
        evaluate(&g, &complete(), 0, &bounds(5, false)),
        LoopDecision::GracefulComplete
    );
}

#[test]
fn running_path_criteria_unmet_keeps_reflecting() {
    // Criteria not yet met, under bound => keep reflecting.
    let g = goal("running");
    assert_eq!(
        evaluate(&g, &blocked(), 1, &bounds(5, false)),
        LoopDecision::Continue
    );
}

#[test]
fn bound_path_stuck_non_perpetual_goal_yields() {
    // Stuck non-perpetual goal past the no-progress bound => yield (no false done).
    let g = goal("stuck");
    let decision = evaluate(&g, &blocked(), 5, &bounds(5, false));
    assert_eq!(decision, LoopDecision::BoundExceeded);
    // Crucially, BoundExceeded is NOT a completion.
    assert_ne!(decision, LoopDecision::GracefulComplete);
}

#[test]
fn perpetual_goal_exempt_from_bound() {
    let g = standing_goal("research");
    assert!(g.is_perpetual());
    assert_eq!(
        evaluate(&g, &blocked(), 100_000, &bounds(5, false)),
        LoopDecision::Continue
    );
}

#[test]
fn perpetual_default_bounds_never_spin_cap() {
    // Default bounds disable the cap => a non-perpetual stuck goal still Continues.
    let g = goal("uncapped");
    assert_eq!(
        evaluate(&g, &blocked(), 100_000, &ReflectionBounds::default()),
        LoopDecision::Continue
    );
}

#[test]
fn board_all_achieved_only_when_every_goal_verified() {
    let mut board = GoalBoard::new();
    board.active.push(goal("a"));
    board.active.push(goal("b"));

    let mut verdicts: BTreeMap<String, CompletionVerdict> = BTreeMap::new();
    verdicts.insert("a".to_string(), complete());
    // b not yet complete.
    verdicts.insert("b".to_string(), blocked());
    assert!(!goals_all_achieved(&board, &verdicts));

    // Now b is verified complete too.
    verdicts.insert("b".to_string(), complete());
    assert!(goals_all_achieved(&board, &verdicts));
}

#[test]
fn from_env_defaults_are_perpetual_safe() {
    // Regardless of ambient env in CI, the parsed-from-values default is perpetual.
    let d = ReflectionBounds::from_env_values(None, None);
    assert_eq!(d.max_reflection_cycles, 0);
    assert!(!d.stop_when_idle);
}
