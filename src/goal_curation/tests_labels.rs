//! Tests (issue #2743): the additive `labels` field on the goal carriers, the
//! `ActiveGoal` label builders, serde back-compatibility, and the deterministic
//! `source:*` provenance stamped at the `operations`/`decompose` creation
//! sites. The pure `labels` brick has its own unit tests in `labels.rs`; these
//! pin the *carriers* and the *creation-path* wiring.

use super::labels::{SOURCE_MEETING, SOURCE_OODA, SOURCE_OPERATOR, SOURCE_OVERSEER, SOURCE_SEED};
use super::operations::{
    active_goals_as_records, add_backlog_item, promote_to_active, seed_default_board,
    update_goal_progress,
};
use super::types::{ActiveGoal, BacklogItem, GoalBoard, GoalNode, GoalProgress};

// ── ActiveGoal: default + builders ─────────────────────────────────────────

#[test]
fn active_goal_new_starts_with_empty_labels() {
    let g = ActiveGoal::new("g", "d", 1);
    assert!(
        g.labels.is_empty(),
        "a fresh goal carries no labels by default"
    );
}

#[test]
fn with_labels_replaces_and_with_label_is_idempotent_and_ordered() {
    let g = ActiveGoal::new("g", "d", 1)
        .with_labels(vec!["source:seed".to_string(), "area:a".to_string()]);
    assert_eq!(g.labels, vec!["source:seed", "area:a"]);

    // with_labels replaces wholesale.
    let g = g.with_labels(vec!["area:b".to_string()]);
    assert_eq!(g.labels, vec!["area:b"]);

    // with_label appends, is order-preserving, and idempotent (dedup + trim).
    let g = g
        .with_label("area:c")
        .with_label("area:b")
        .with_label("  area:c  ");
    assert_eq!(g.labels, vec!["area:b", "area:c"]);
}

// ── serde back-compat: additive, no migration ──────────────────────────────

#[test]
fn active_goal_legacy_json_without_labels_loads_empty() {
    // A pre-#2743 board snapshot has no `labels` key. serde(default) fills it.
    let legacy =
        r#"{"id":"g","description":"d","priority":2,"status":"NotStarted","assigned_to":null}"#;
    let g: ActiveGoal = serde_json::from_str(legacy).expect("legacy goal deserializes");
    assert!(g.labels.is_empty(), "missing labels key -> empty Vec");
}

#[test]
fn unlabelled_active_goal_serializes_without_labels_key() {
    // skip_serializing_if keeps an unlabelled goal byte-identical to its
    // pre-#2743 form, so the board snapshot hash is unchanged.
    let g = ActiveGoal::new("g", "d", 1);
    let json = serde_json::to_string(&g).expect("serialize");
    assert!(!json.contains("\"labels\""), "empty labels omitted: {json}");
}

#[test]
fn labelled_active_goal_round_trips_and_emits_key() {
    let g = ActiveGoal::new("g", "d", 1)
        .with_label("source:operator")
        .with_label("area:x");
    let json = serde_json::to_string(&g).expect("serialize");
    assert!(
        json.contains("\"labels\""),
        "non-empty labels are serialized"
    );
    let back: ActiveGoal = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.labels, vec!["source:operator", "area:x"]);
    assert_eq!(back, g, "full round-trip is lossless");
}

#[test]
fn goal_node_labels_serde_back_compatible() {
    // Legacy node (no labels key) loads empty.
    let legacy = r#"{"id":"n","description":"d"}"#;
    let node: GoalNode = serde_json::from_str(legacy).expect("legacy node deserializes");
    assert!(node.labels.is_empty());

    // Unlabelled node omits the key; labelled node round-trips.
    let bare = GoalNode::new("n", "d", None::<String>, Vec::new());
    assert!(!serde_json::to_string(&bare).unwrap().contains("\"labels\""));

    let tagged = GoalNode::new("n", "d", None::<String>, vec!["source:seed".to_string()]);
    let round: GoalNode =
        serde_json::from_str(&serde_json::to_string(&tagged).unwrap()).expect("round-trip");
    assert_eq!(round.labels, vec!["source:seed"]);
}

// ── provenance stamping at the operations creation sites ───────────────────

#[test]
fn seed_default_board_stamps_source_seed_on_every_goal() {
    let mut board = GoalBoard::new();
    let added = seed_default_board(&mut board);
    assert!(added > 0, "seed adds goals to an empty board");
    assert!(
        board
            .active
            .iter()
            .all(|g| g.labels.iter().any(|l| l == SOURCE_SEED)),
        "every seeded goal is stamped source:seed",
    );
}

#[test]
fn promote_to_active_stamps_source_from_backlog_prefix() {
    // Each backlog source prefix maps to its provenance tag on promotion (the
    // item's first label-bearing materialization); unknown -> source:ooda.
    let cases = [
        ("operator:demote", SOURCE_OPERATOR),
        ("meeting:dashboard owner=al", SOURCE_MEETING),
        ("overseer:amplihack-rs", SOURCE_OVERSEER),
        ("stewardship:repo#1", SOURCE_OODA),
    ];
    for (i, (source, expected)) in cases.iter().enumerate() {
        let mut board = GoalBoard::new();
        let id = format!("bk-{i}");
        add_backlog_item(
            &mut board,
            BacklogItem {
                id: id.clone(),
                description: format!("backlog {i}"),
                source: (*source).to_string(),
                score: 0.5,
            },
        )
        .expect("enqueue backlog item");

        promote_to_active(&mut board, &id, 3, None).expect("promote");
        let goal = board
            .active
            .iter()
            .find(|g| g.id == id)
            .expect("promoted goal");
        assert_eq!(
            goal.labels,
            vec![expected.to_string()],
            "backlog source '{source}' must stamp {expected} on promotion",
        );
    }
}

#[test]
fn active_goals_as_records_preserves_labels() {
    // The ActiveGoal -> GoalRecord projection must carry provenance through.
    let mut board = GoalBoard::new();
    board.active.push(
        ActiveGoal::new("g", "Do the thing", 1)
            .with_label("source:creative-ideas")
            .with_label("area:dashboard"),
    );
    let records = active_goals_as_records(&board);
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].labels,
        vec!["source:creative-ideas", "area:dashboard"]
    );
}

#[test]
fn in_place_lifecycle_move_does_not_restamp_labels() {
    // Unblocking / progress updates are in-place moves, NOT creation, so they
    // must never re-stamp or drop a goal's source:* provenance.
    let mut board = GoalBoard::new();
    board.active.push(
        ActiveGoal::new("g", "Do the thing", 1)
            .with_label("source:creative-ideas")
            .with_label("area:x"),
    );
    let before = board.active[0].labels.clone();

    update_goal_progress(&mut board, "g", GoalProgress::InProgress { percent: 40 })
        .expect("progress update");
    // Simulate an unblock (status flip) directly on the board.
    board.active[0].status = GoalProgress::NotStarted;

    assert_eq!(
        board.active[0].labels, before,
        "an in-place lifecycle move leaves labels (and provenance) untouched",
    );
}
