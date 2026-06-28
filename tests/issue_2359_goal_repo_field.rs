//! Integration tests for the per-goal target-repo field (issue #2359, BUG 1).
//!
//! Contract reference: `docs/reference/goal-target-repo-routing.md`.
//!
//! # TDD red-phase note
//!
//! These tests pin the public contract of the new `ActiveGoal::repo` field,
//! the `ActiveGoal::new`/`with_repo` constructors, the serde back-compat
//! attributes, and the `DEFAULT_SEED_GOALS` 4-tuple arity. Until the
//! implementation step adds those, this test target **fails to compile** —
//! that is the intended red state. It must compile and pass once the field
//! and seed changes land, without further test edits.
//!
//! These live in `tests/` (not inline) so the field migration across the
//! ~100 `ActiveGoal { .. }` literal construction sites belongs to the
//! implementation step, not the test step.

use simard::goal_curation::{
    ActiveGoal, DEFAULT_SEED_GOALS, GoalBoard, GoalProgress, seed_default_board,
};

// ── ActiveGoal::repo field + constructors ─────────────────────────────────

#[test]
fn active_goal_new_defaults_repo_to_none() {
    let g = ActiveGoal::new("goal-x", "do the thing", 3);
    assert_eq!(g.id, "goal-x");
    assert_eq!(g.description, "do the thing");
    assert_eq!(g.priority, 3);
    assert_eq!(g.status, GoalProgress::NotStarted);
    assert_eq!(g.assigned_to, None);
    assert_eq!(
        g.repo, None,
        "a freshly constructed goal targets the daemon repo (repo = None)"
    );
}

#[test]
fn with_repo_sets_the_target_slug() {
    let g =
        ActiveGoal::new("goal-x", "do the thing", 3).with_repo(Some("amplihack-rs".to_string()));
    assert_eq!(g.repo.as_deref(), Some("amplihack-rs"));

    let cleared = g.with_repo(None);
    assert_eq!(cleared.repo, None, "with_repo(None) clears the target repo");
}

// ── serde back-compatibility ──────────────────────────────────────────────

#[test]
fn deserializes_pre_2359_goal_without_repo_key_to_none() {
    // Goal-board JSON written before #2359 has no `repo` key.
    let legacy = r#"{
        "id": "legacy-goal",
        "description": "an older goal",
        "priority": 2,
        "status": "NotStarted",
        "assigned_to": null
    }"#;

    let g: ActiveGoal = serde_json::from_str(legacy)
        .expect("legacy goal JSON without a repo key must still deserialize");
    assert_eq!(
        g.repo, None,
        "#[serde(default)] must deserialize a missing repo key to None"
    );
}

#[test]
fn repo_none_is_omitted_from_serialized_json() {
    let g = ActiveGoal::new("repo-less", "no target repo", 1);
    let json = serde_json::to_value(&g).expect("serialize");
    assert!(
        json.get("repo").is_none(),
        "skip_serializing_if = Option::is_none must omit the repo key entirely \
         for repo-less goals (byte-identical pre-#2359 snapshots), got: {json}"
    );
}

#[test]
fn repo_some_round_trips_through_json() {
    let g = ActiveGoal::new("targeted", "targets amplihack-rs", 1)
        .with_repo(Some("amplihack-rs".to_string()));
    let json = serde_json::to_value(&g).expect("serialize");
    assert_eq!(
        json.get("repo").and_then(|v| v.as_str()),
        Some("amplihack-rs"),
        "a goal with a repo must serialize the slug"
    );

    let back: ActiveGoal = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, g, "a goal with a repo must round-trip unchanged");
}

// ── Seed goals carry the correct target repo ──────────────────────────────

#[test]
fn default_seed_goals_tuple_carries_repo_slug() {
    // The tuple gains a 4th element: the optional target-repo slug.
    let amplihack = DEFAULT_SEED_GOALS
        .iter()
        .find(|(_priority, title, _desc, _repo)| title.to_lowercase().contains("amplihack"))
        .expect("a seed goal about amplihack must exist");
    let (_priority, _title, _desc, repo) = amplihack;
    assert_eq!(
        *repo,
        Some("amplihack-rs"),
        "the amplihack test-coverage seed goal must target the amplihack-rs repo"
    );
}

#[test]
fn seed_default_board_threads_repo_onto_active_goals() {
    let mut board = GoalBoard::new();
    let added = seed_default_board(&mut board);
    assert_eq!(added, DEFAULT_SEED_GOALS.len());

    let amplihack_goal = board
        .active
        .iter()
        .find(|g| g.description.to_lowercase().contains("amplihack"))
        .expect("seeded board must contain the amplihack goal");
    assert_eq!(
        amplihack_goal.repo.as_deref(),
        Some("amplihack-rs"),
        "the seeded amplihack goal must route to the amplihack-rs repo"
    );

    // A Simard-targeted seed goal keeps repo = None.
    let simard_goal = board
        .active
        .iter()
        .find(|g| g.description.to_lowercase().contains("meeting"))
        .expect("seeded board must contain the meeting-experience goal");
    assert_eq!(
        simard_goal.repo, None,
        "a Simard-targeted seed goal must keep repo = None"
    );
}
