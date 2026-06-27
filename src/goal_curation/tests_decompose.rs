//! Tests (issue #2405): `decompose_goal`, the parent-linkage data model,
//! and parent-progress roll-up.
//!
//! These pin the behavior of the shipped increment:
//!   - `ActiveGoal::parent_goal_id` + `ActiveGoal::with_parent`
//!   - `super::decompose::{decompose_goal, GoalDecomposer, SubGoalProposal,
//!      ChildPlacement, DecomposeOutcome}`
//!   - `super::operations::rollup_parent_progress`
//!
//! Decomposition is exercised against the **real** library backend
//! ([`LibraryCognitiveMemory::in_memory`]) so the edge writes it performs are
//! genuinely queryable back via [`super::edges::children_of`].

use super::decompose::{
    ChildPlacement, DecomposeOutcome, GoalDecomposer, SubGoalProposal, decompose_goal,
};
use super::edges::children_of;
use super::operations::rollup_parent_progress;
use super::types::{ActiveGoal, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS};
use crate::cognitive_memory::LibraryCognitiveMemory;
use crate::error::{SimardError, SimardResult};

fn mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory cognitive memory")
}

fn sub(desc: &str, done: &str) -> SubGoalProposal {
    SubGoalProposal {
        description: desc.to_string(),
        done_criterion: done.to_string(),
        depends_on: vec![],
    }
}

/// Canned decomposer: returns a fixed proposal list, or a simulated failure.
/// `decompose_goal` — not the decomposer — is responsible for enforcing the
/// `2..=6` bound, so this stub returns whatever it is given verbatim.
struct CannedDecomposer {
    proposals: Vec<SubGoalProposal>,
    fail: bool,
}

impl CannedDecomposer {
    fn ok(proposals: Vec<SubGoalProposal>) -> Self {
        Self {
            proposals,
            fail: false,
        }
    }
    fn failing() -> Self {
        Self {
            proposals: vec![],
            fail: true,
        }
    }
}

impl GoalDecomposer for CannedDecomposer {
    fn propose_subgoals(
        &self,
        _parent: &ActiveGoal,
        _max_children: usize,
    ) -> SimardResult<Vec<SubGoalProposal>> {
        if self.fail {
            return Err(SimardError::InvalidGoalRecord {
                field: "decomposer".to_string(),
                reason: "simulated LLM failure".to_string(),
            });
        }
        Ok(self.proposals.clone())
    }
}

fn board_with_parent() -> GoalBoard {
    let mut board = GoalBoard::new();
    board
        .active
        .push(ActiveGoal::new("goal-p", "Big umbrella goal", 1));
    board
}

// ── Data model: parent linkage on ActiveGoal ───────────────────────────────

#[test]
fn active_goal_defaults_to_no_parent() {
    let g = ActiveGoal::new("c", "d", 1);
    assert_eq!(g.parent_goal_id, None, "a top-level goal has no parent");
}

#[test]
fn with_parent_sets_linkage() {
    let g = ActiveGoal::new("c", "d", 1).with_parent(Some("goal-p".to_string()));
    assert_eq!(g.parent_goal_id.as_deref(), Some("goal-p"));
}

#[test]
fn parent_goal_id_omitted_from_json_when_none() {
    let g = ActiveGoal::new("c", "d", 1);
    let json = serde_json::to_string(&g).unwrap();
    assert!(
        !json.contains("parent_goal_id"),
        "None parent must be skipped so pre-#2405 snapshots stay byte-identical: {json}"
    );
}

#[test]
fn parent_goal_id_present_in_json_when_set() {
    let g = ActiveGoal::new("c", "d", 1).with_parent(Some("goal-p".to_string()));
    let json = serde_json::to_string(&g).unwrap();
    assert!(
        json.contains(r#""parent_goal_id":"goal-p""#),
        "a set parent must serialize: {json}"
    );
}

#[test]
fn legacy_goal_without_parent_field_deserializes_to_none() {
    // A pre-#2405 goal-board snapshot / goal_records.json entry has no
    // `parent_goal_id` key. `#[serde(default)]` must let it load unchanged.
    let legacy =
        r#"{"id":"g","description":"d","priority":1,"status":"NotStarted","assigned_to":null}"#;
    let g: ActiveGoal = serde_json::from_str(legacy).expect("legacy goal must still deserialize");
    assert_eq!(g.parent_goal_id, None);
    assert_eq!(g.id, "g");
}

// ── decompose_goal: happy path ─────────────────────────────────────────────

#[test]
fn decompose_writes_children_and_queryable_edges() {
    let m = mem();
    let mut board = board_with_parent();
    let decomposer = CannedDecomposer::ok(vec![
        sub("Slice A", "A is done"),
        sub("Slice B", "B is done"),
        sub("Slice C", "C is done"),
    ]);

    let outcome: DecomposeOutcome =
        decompose_goal(&m, &mut board, "goal-p", &decomposer, 6).expect("decompose succeeds");

    assert_eq!(outcome.parent_id, "goal-p");
    assert_eq!(
        outcome.child_ids.len(),
        3,
        "three sub-goals -> three children"
    );
    assert_eq!(
        outcome.placement,
        ChildPlacement::Board,
        "with room on the board, children replace the parent on the board"
    );

    // The parent → child edges round-trip back out of the graph.
    let mut kids = children_of(&m, "goal-p").unwrap();
    kids.sort();
    let mut expected = outcome.child_ids.clone();
    expected.sort();
    assert_eq!(
        kids, expected,
        "every child must be queryable from the parent"
    );

    // Children placed on the board carry the cheap in-board back-reference.
    for cid in &outcome.child_ids {
        let child = board
            .active
            .iter()
            .find(|g| &g.id == cid)
            .expect("child must be on the active board");
        assert_eq!(
            child.parent_goal_id.as_deref(),
            Some("goal-p"),
            "board children must carry parent_goal_id"
        );
    }

    // The parent is replaced by its children on the active board.
    assert!(
        board.active.iter().all(|g| g.id != "goal-p"),
        "parent is replaced by its children on the board"
    );
}

// ── decompose_goal: fan-out bounds [2, 6] ──────────────────────────────────

#[test]
fn decompose_rejects_fewer_than_two_subgoals() {
    let m = mem();
    let mut board = board_with_parent();
    let before = board.clone();
    let decomposer = CannedDecomposer::ok(vec![sub("only one", "x")]);

    let res = decompose_goal(&m, &mut board, "goal-p", &decomposer, 6);

    assert!(
        res.is_err(),
        "a single sub-goal is not a real decomposition (loud fallback, not a spin)"
    );
    assert_eq!(board, before, "board must be untouched on fallback");
    assert!(
        children_of(&m, "goal-p").unwrap().is_empty(),
        "no edges may be written on fallback"
    );
}

#[test]
fn decompose_clamps_fanout_to_six() {
    let m = mem();
    let mut board = board_with_parent();
    let many: Vec<SubGoalProposal> = (0..8)
        .map(|i| sub(&format!("Slice {i}"), &format!("done {i}")))
        .collect();
    let decomposer = CannedDecomposer::ok(many);

    let outcome = decompose_goal(&m, &mut board, "goal-p", &decomposer, 6).unwrap();

    assert_eq!(
        outcome.child_ids.len(),
        6,
        "fan-out is clamped to a maximum of six children"
    );
    assert_eq!(children_of(&m, "goal-p").unwrap().len(), 6);
}

#[test]
fn decompose_caps_effective_max_children_regardless_of_caller() {
    let m = mem();
    let mut board = board_with_parent();
    let many: Vec<SubGoalProposal> = (0..8)
        .map(|i| sub(&format!("Slice {i}"), &format!("done {i}")))
        .collect();
    let decomposer = CannedDecomposer::ok(many);

    // Caller asks for 100; the capability must still cap at 6.
    let outcome = decompose_goal(&m, &mut board, "goal-p", &decomposer, 100).unwrap();
    assert_eq!(outcome.child_ids.len(), 6);
}

#[test]
fn decompose_floors_effective_max_children_to_two() {
    let m = mem();
    let mut board = board_with_parent();
    let decomposer = CannedDecomposer::ok(vec![
        sub("Slice A", "A done"),
        sub("Slice B", "B done"),
        sub("Slice C", "C done"),
    ]);

    // Caller asks for 1, which is nonsensical for a decomposition; floor to 2.
    let outcome = decompose_goal(&m, &mut board, "goal-p", &decomposer, 1).unwrap();
    assert_eq!(
        outcome.child_ids.len(),
        2,
        "max_children is floored to two so a decomposition always yields >= 2 children"
    );
}

// ── decompose_goal: deterministic loud fallback ────────────────────────────

#[test]
fn decompose_propagates_decomposer_failure_without_mutating_state() {
    let m = mem();
    let mut board = board_with_parent();
    let before = board.clone();
    let decomposer = CannedDecomposer::failing();

    let res = decompose_goal(&m, &mut board, "goal-p", &decomposer, 6);

    assert!(res.is_err(), "a failed decomposition must surface loudly");
    assert_eq!(
        board, before,
        "board must be left intact when decomposition fails"
    );
    assert!(
        children_of(&m, "goal-p").unwrap().is_empty(),
        "no partial edges may leak on failure"
    );
}

#[test]
fn decompose_unknown_goal_errors() {
    let m = mem();
    let mut board = board_with_parent();
    let decomposer = CannedDecomposer::ok(vec![sub("a", "x"), sub("b", "y")]);

    let res = decompose_goal(&m, &mut board, "ghost-goal", &decomposer, 6);
    assert!(res.is_err(), "decomposing a non-existent goal must error");
}

// ── decompose_goal: active-cap overflow → backlog, edges still written ──────

#[test]
fn decompose_overflows_to_backlog_but_still_writes_edges() {
    let m = mem();
    // Parent + 5 unrelated active goals = 6 active. Removing the parent frees
    // one slot (5 left); adding 3 children -> 8 > MAX_ACTIVE_GOALS (7), so the
    // children cannot all be promoted and must overflow to the backlog.
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal::new("goal-p", "Umbrella", 1));
    for i in 0..5 {
        board.active.push(ActiveGoal::new(
            format!("other-{i}"),
            format!("Other {i}"),
            2,
        ));
    }
    assert_eq!(board.active.len(), 6);

    let decomposer = CannedDecomposer::ok(vec![sub("a", "x"), sub("b", "y"), sub("c", "z")]);

    let outcome = decompose_goal(&m, &mut board, "goal-p", &decomposer, 6).unwrap();

    assert_eq!(
        outcome.placement,
        ChildPlacement::Backlog,
        "children overflow to the backlog when the active cap would be exceeded"
    );
    assert_eq!(outcome.child_ids.len(), 3);

    // THE key guarantee: edges are written regardless of placement, so a child
    // sitting in the backlog is still a queryable child of its parent.
    let mut kids = children_of(&m, "goal-p").unwrap();
    kids.sort();
    let mut expected = outcome.child_ids.clone();
    expected.sort();
    assert_eq!(
        kids, expected,
        "edges written even when children overflow to backlog"
    );

    // Children are in the backlog, not on the active board.
    for cid in &outcome.child_ids {
        assert!(
            board.backlog.iter().any(|b| &b.id == cid),
            "overflow child {cid} must be in the backlog"
        );
        assert!(
            board.active.iter().all(|g| &g.id != cid),
            "overflow child {cid} must not be on the active board"
        );
    }

    // The active cap is never exceeded, and the parent stays as the anchor.
    assert!(board.active.len() <= MAX_ACTIVE_GOALS);
    assert!(
        board.active.iter().any(|g| g.id == "goal-p"),
        "parent stays active as the roll-up anchor on overflow"
    );
}

// ── Roll-up: parent progress from children ─────────────────────────────────

fn child(status: GoalProgress) -> ActiveGoal {
    let mut g = ActiveGoal::new("child", "desc", 1);
    g.status = status;
    g
}

#[test]
fn rollup_of_no_children_is_none() {
    assert_eq!(
        rollup_parent_progress(&[]),
        None,
        "a goal with no children rolls up to its own directly-tracked status (None signals 'keep own')"
    );
}

#[test]
fn rollup_all_completed_is_completed() {
    let kids = [
        child(GoalProgress::Completed),
        child(GoalProgress::Completed),
    ];
    assert_eq!(rollup_parent_progress(&kids), Some(GoalProgress::Completed));
}

#[test]
fn rollup_completed_and_not_started_is_fifty_percent() {
    let kids = [
        child(GoalProgress::Completed),
        child(GoalProgress::NotStarted),
    ];
    assert_eq!(
        rollup_parent_progress(&kids),
        Some(GoalProgress::InProgress { percent: 50 })
    );
}

#[test]
fn rollup_in_progress_children_average() {
    let kids = [
        child(GoalProgress::InProgress { percent: 50 }),
        child(GoalProgress::InProgress { percent: 100 }),
    ];
    assert_eq!(
        rollup_parent_progress(&kids),
        Some(GoalProgress::InProgress { percent: 75 })
    );
}

#[test]
fn rollup_all_not_started_is_zero_percent_not_completed() {
    let kids = [
        child(GoalProgress::NotStarted),
        child(GoalProgress::NotStarted),
    ];
    assert_eq!(
        rollup_parent_progress(&kids),
        Some(GoalProgress::InProgress { percent: 0 }),
        "not-started children roll up to 0%, never to Completed"
    );
}

#[test]
fn rollup_paused_and_proposed_count_as_zero() {
    assert_eq!(
        rollup_parent_progress(&[child(GoalProgress::Paused), child(GoalProgress::Completed)]),
        Some(GoalProgress::InProgress { percent: 50 })
    );
    assert_eq!(
        rollup_parent_progress(&[
            child(GoalProgress::Proposed),
            child(GoalProgress::Completed)
        ]),
        Some(GoalProgress::InProgress { percent: 50 })
    );
}

#[test]
fn rollup_rounds_the_mean() {
    // (0 + 0 + 100) / 3 = 33.33 -> 33 under both floor and round-to-nearest.
    let kids = [
        child(GoalProgress::NotStarted),
        child(GoalProgress::NotStarted),
        child(GoalProgress::Completed),
    ];
    assert_eq!(
        rollup_parent_progress(&kids),
        Some(GoalProgress::InProgress { percent: 33 })
    );
}

#[test]
fn rollup_surfaces_a_blocked_child() {
    let kids = [
        child(GoalProgress::Completed),
        child(GoalProgress::Blocked("waiting on review".to_string())),
    ];
    match rollup_parent_progress(&kids) {
        Some(GoalProgress::Blocked(_)) => {}
        other => panic!("a blocked child must surface the block on the parent, got {other:?}"),
    }
}
