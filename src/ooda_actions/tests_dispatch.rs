//! Unit tests for the top-level `dispatch_actions` orchestrator and
//! the `make_outcome` helper in ooda_actions.

use crate::ooda_actions::dispatch_actions;
use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};

use super::make_outcome;
use super::test_helpers::{board_with_goal, test_bridges};
use crate::goal_curation::GoalProgress;

// Simard #3125: the deterministic read-only spawn rail. `dispatch_spawn_engineer`
// is the single write-bearing chokepoint the Act phase funnels through; the
// rail is the L2 defense-in-depth layer that hard-blocks it when the active
// identity's posture is read-only.
use crate::identity::WriteAuthority;
use crate::ooda_actions::advance_goal::spawn::dispatch_spawn_engineer;
use crate::ooda_brain::{DeterministicAdmissionBrain, DeterministicLifecycleBrain};
use std::path::Path;
use std::sync::Mutex;

// ── make_outcome ────────────────────────────────────────────────

#[test]
fn make_outcome_success_preserves_fields() {
    let action = PlannedAction {
        kind: ActionKind::ConsolidateMemory,
        goal_id: None,
        description: "consolidate all memory".to_string(),
    };
    let outcome = make_outcome(&action, true, "done".to_string());
    assert!(outcome.success);
    assert_eq!(outcome.detail, "done");
    assert_eq!(outcome.action.kind, ActionKind::ConsolidateMemory);
    assert_eq!(outcome.action.description, "consolidate all memory");
}

#[test]
fn make_outcome_failure_preserves_fields() {
    let action = PlannedAction {
        kind: ActionKind::RunGymEval,
        goal_id: None,
        description: "run gym".to_string(),
    };
    let outcome = make_outcome(&action, false, "timeout".to_string());
    assert!(!outcome.success);
    assert_eq!(outcome.detail, "timeout");
}

#[test]
fn make_outcome_clones_action_independently() {
    let action = PlannedAction {
        kind: ActionKind::ResearchQuery,
        goal_id: Some("g1".to_string()),
        description: "research".to_string(),
    };
    let outcome = make_outcome(&action, true, "ok".to_string());
    assert_eq!(outcome.action.goal_id, Some("g1".to_string()));
}

// ── dispatch_actions ────────────────────────────────────────────

#[test]
fn dispatch_empty_actions_returns_empty_vec() {
    let mut bridges = test_bridges();
    let board = board_with_goal("g1", GoalProgress::NotStarted, None);
    let mut state = OodaState::new(board);
    let outcomes = dispatch_actions(&[], &mut bridges, &mut state).unwrap();
    assert!(outcomes.is_empty());
}

#[test]
fn dispatch_consolidate_memory_returns_one_outcome() {
    let mut bridges = test_bridges();
    let board = board_with_goal("g1", GoalProgress::NotStarted, None);
    let mut state = OodaState::new(board);
    let actions = vec![PlannedAction {
        kind: ActionKind::ConsolidateMemory,
        goal_id: None,
        description: "consolidate".to_string(),
    }];
    let outcomes = dispatch_actions(&actions, &mut bridges, &mut state).unwrap();
    assert_eq!(outcomes.len(), 1);
    assert!(outcomes[0].success);
}

#[test]
fn dispatch_research_query_returns_one_outcome() {
    let mut bridges = test_bridges();
    let board = board_with_goal("g1", GoalProgress::NotStarted, None);
    let mut state = OodaState::new(board);
    let actions = vec![PlannedAction {
        kind: ActionKind::ResearchQuery,
        goal_id: None,
        description: "look up patterns".to_string(),
    }];
    let outcomes = dispatch_actions(&actions, &mut bridges, &mut state).unwrap();
    assert_eq!(outcomes.len(), 1);
    assert!(outcomes[0].success);
}

#[test]
fn dispatch_multiple_independent_actions_preserves_order() {
    let mut bridges = test_bridges();
    let board = board_with_goal("g1", GoalProgress::NotStarted, None);
    let mut state = OodaState::new(board);
    let actions = vec![
        PlannedAction {
            kind: ActionKind::ConsolidateMemory,
            goal_id: None,
            description: "consolidate".to_string(),
        },
        PlannedAction {
            kind: ActionKind::RunGymEval,
            goal_id: None,
            description: "gym eval".to_string(),
        },
    ];
    let outcomes = dispatch_actions(&actions, &mut bridges, &mut state).unwrap();
    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0].action.description, "consolidate");
    assert_eq!(outcomes[1].action.description, "gym eval");
}

#[test]
fn dispatch_advance_goal_without_session_fails_gracefully() {
    let mut bridges = test_bridges(); // no session
    let board = board_with_goal("g1", GoalProgress::InProgress { percent: 30 }, None);
    let mut state = OodaState::new(board);
    let actions = vec![PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some("g1".to_string()),
        description: "advance goal".to_string(),
    }];
    let outcomes = dispatch_actions(&actions, &mut bridges, &mut state).unwrap();
    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].success);
}

// ── Simard #3125: deterministic read-only spawn rail (L2) ────────────────────
//
// AC3/AC5: when the active identity's posture is read-only,
// `dispatch_spawn_engineer` MUST hard-block — returning a benign
// `success == true` skip (so the 3-strikes brain-failure safeguard is not
// tripped) WITHOUT allocating a worktree, assigning a subordinate, or spawning
// any process. This is defense-in-depth beneath the observe-only Act branch:
// even if control reaches the write chokepoint, the rail refuses the write.
//
// The rail is expected at the TOP of `dispatch_spawn_engineer` (before the
// assignment re-check / admission gate), so these tests pass throwaway
// deterministic brains that must never be consulted on the read-only path.

fn spawn_action(goal_id: &str) -> PlannedAction {
    PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some(goal_id.to_string()),
        description: format!("advance {goal_id}"),
    }
}

#[test]
fn read_only_posture_blocks_spawn_engineer_without_assigning() {
    let action = spawn_action("obs-goal");
    let mut state = OodaState::new(board_with_goal("obs-goal", GoalProgress::NotStarted, None));
    // Flip the daemon-resolved posture to read-only (the observer case).
    state.write_authority = WriteAuthority::ReadOnly;

    let state_mx = Mutex::new(&mut state);
    let brain = DeterministicLifecycleBrain;
    let admission = DeterministicAdmissionBrain;

    let outcome = dispatch_spawn_engineer(
        &action,
        &state_mx,
        "obs-goal",
        "do the (blocked) work",
        &brain,
        &admission,
        Path::new("."),
    );

    // Benign skip: success=true so the failure counter is NOT bumped.
    assert!(
        outcome.success,
        "read-only spawn block must be a benign skip, not a failure: {}",
        outcome.detail
    );
    let detail = outcome.detail.to_lowercase();
    assert!(
        detail.contains("read-only") || detail.contains("read only") || detail.contains("observe"),
        "skip detail must name the read-only / observe-only posture: {}",
        outcome.detail
    );

    // Crucially: NO engineer was dispatched — the goal is still unassigned.
    // End the `state_mx` borrow of `state` before inspecting the board.
    let state = state_mx.into_inner().expect("state mutex not poisoned");
    let goal = state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == "obs-goal")
        .expect("goal present");
    assert!(
        goal.assigned_to.is_none(),
        "read-only posture must never assign a subordinate engineer"
    );
}

#[test]
fn read_write_posture_does_not_trigger_read_only_rail() {
    // AC1: Simard herself (read-write) is unaffected by the rail. With an
    // already-assigned goal, dispatch falls THROUGH the rail to the normal
    // assignment re-check and reports the ordinary "already assigned" skip —
    // proving the rail did not short-circuit for read-write. (Using an
    // already-assigned goal keeps the test from spawning a real subprocess.)
    let action = spawn_action("rw-goal");
    let mut state = OodaState::new(board_with_goal(
        "rw-goal",
        GoalProgress::InProgress { percent: 30 },
        Some("subordinate-1"),
    ));
    state.write_authority = WriteAuthority::ReadWrite;

    let state_mx = Mutex::new(&mut state);
    let brain = DeterministicLifecycleBrain;
    let admission = DeterministicAdmissionBrain;

    let outcome = dispatch_spawn_engineer(
        &action,
        &state_mx,
        "rw-goal",
        "do the work",
        &brain,
        &admission,
        Path::new("."),
    );

    assert!(
        outcome.success,
        "already-assigned skip is success: {}",
        outcome.detail
    );
    let detail = outcome.detail.to_lowercase();
    assert!(
        detail.contains("already assigned"),
        "read-write dispatch must reach the assignment re-check, not the read-only rail: {}",
        outcome.detail
    );
    assert!(
        !detail.contains("observe"),
        "read-write dispatch must NOT emit the observe-only read-only skip: {}",
        outcome.detail
    );
}
