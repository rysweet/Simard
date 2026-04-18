//! Unit tests for the top-level `dispatch_actions` orchestrator and
//! the `make_outcome` helper in ooda_actions.

use crate::ooda_actions::dispatch_actions;
use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};

use super::make_outcome;
use super::test_helpers::{board_with_goal, test_bridges};
use crate::goal_curation::GoalProgress;

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
