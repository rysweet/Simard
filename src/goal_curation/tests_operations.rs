use super::operations::*;
use super::types::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS};

fn make_goal(id: &str, priority: u32) -> ActiveGoal {
    ActiveGoal {
        id: id.to_string(),
        description: format!("Goal {id}"),
        priority,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    }
}

fn make_backlog(id: &str) -> BacklogItem {
    BacklogItem {
        id: id.to_string(),
        description: format!("Backlog {id}"),
        source: "test".to_string(),
        score: 0.0,
    }
}

#[test]
fn add_active_goal_succeeds_and_rejects_duplicate() {
    let mut board = GoalBoard::new();
    assert!(add_active_goal(&mut board, make_goal("g1", 1)).is_ok());
    assert_eq!(board.active.len(), 1);
    assert!(add_active_goal(&mut board, make_goal("g1", 2)).is_err());
}

#[test]
fn add_active_goal_rejects_at_capacity() {
    let mut board = GoalBoard::new();
    for i in 0..MAX_ACTIVE_GOALS {
        add_active_goal(&mut board, make_goal(&format!("g{i}"), (i + 1) as u32)).unwrap();
    }
    let result = add_active_goal(&mut board, make_goal("overflow", 1));
    assert!(result.is_err());
}

#[test]
fn add_active_goal_rejects_zero_priority() {
    let mut board = GoalBoard::new();
    let result = add_active_goal(&mut board, make_goal("g1", 0));
    assert!(result.is_err());
}

#[test]
fn add_backlog_item_succeeds_and_rejects_duplicate() {
    let mut board = GoalBoard::new();
    assert!(add_backlog_item(&mut board, make_backlog("b1")).is_ok());
    assert_eq!(board.backlog.len(), 1);
    assert!(add_backlog_item(&mut board, make_backlog("b1")).is_err());
}

#[test]
fn promote_to_active_moves_item() {
    let mut board = GoalBoard::new();
    add_backlog_item(&mut board, make_backlog("b1")).unwrap();
    promote_to_active(&mut board, "b1", 1, None).unwrap();
    assert!(board.backlog.is_empty());
    assert_eq!(board.active.len(), 1);
    assert_eq!(board.active[0].id, "b1");
    assert!(matches!(board.active[0].status, GoalProgress::NotStarted));
}

#[test]
fn promote_to_active_not_found() {
    let mut board = GoalBoard::new();
    assert!(promote_to_active(&mut board, "nonexistent", 1, None).is_err());
}

#[test]
fn update_goal_progress_and_archive_completed() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, make_goal("g1", 1)).unwrap();
    add_active_goal(&mut board, make_goal("g2", 2)).unwrap();
    update_goal_progress(&mut board, "g1", GoalProgress::Completed).unwrap();
    let archived = archive_completed(&mut board);
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, "g1");
    assert_eq!(board.active.len(), 1);
}

#[test]
fn update_goal_progress_rejects_over_100_percent() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, make_goal("g1", 1)).unwrap();
    let result = update_goal_progress(&mut board, "g1", GoalProgress::InProgress { percent: 101 });
    assert!(result.is_err());
}

#[test]
fn seed_default_board_populates_empty_board() {
    let mut board = GoalBoard::new();
    let count = seed_default_board(&mut board);
    assert_eq!(count, DEFAULT_SEED_GOALS.len());
    assert_eq!(board.active.len(), DEFAULT_SEED_GOALS.len());
}

#[test]
fn seed_default_board_skips_non_empty() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, make_goal("existing", 1)).unwrap();
    let count = seed_default_board(&mut board);
    assert_eq!(count, 0);
    assert_eq!(board.active.len(), 1);
}

// ── enqueue_stewardship_issue (issue #1167) ─────────────────────────

#[test]
fn enqueue_stewardship_issue_adds_backlog_row() {
    let mut board = GoalBoard::new();
    super::enqueue_stewardship_issue(
        &mut board,
        "rysweet/Simard",
        42,
        "https://github.com/rysweet/Simard/issues/42",
        "abcdef0123456789",
    )
    .unwrap();
    assert_eq!(board.backlog.len(), 1);
    let item = &board.backlog[0];
    assert_eq!(item.id, "stewardship-rysweet_Simard-42");
    assert_eq!(item.source, "stewardship:rysweet/Simard#42");
    assert!(item.description.contains("abcdef0123456789"));
    assert!(
        item.description
            .contains("https://github.com/rysweet/Simard/issues/42")
    );
    assert!(item.score > 0.0 && item.score <= 1.0);
}

#[test]
fn enqueue_stewardship_issue_is_idempotent_on_same_issue() {
    let mut board = GoalBoard::new();
    super::enqueue_stewardship_issue(
        &mut board,
        "rysweet/Simard",
        42,
        "https://github.com/rysweet/Simard/issues/42",
        "sig",
    )
    .unwrap();
    // Second call with same (repo, issue#) → no-op (returns Ok, backlog unchanged).
    super::enqueue_stewardship_issue(
        &mut board,
        "rysweet/Simard",
        42,
        "https://github.com/rysweet/Simard/issues/42",
        "sig",
    )
    .unwrap();
    assert_eq!(board.backlog.len(), 1, "must not duplicate stewardship row");
}

#[test]
fn enqueue_stewardship_issue_amplihack_repo() {
    let mut board = GoalBoard::new();
    super::enqueue_stewardship_issue(
        &mut board,
        "rysweet/amplihack",
        7,
        "https://github.com/rysweet/amplihack/issues/7",
        "deadbeef",
    )
    .unwrap();
    let item = &board.backlog[0];
    assert_eq!(item.id, "stewardship-rysweet_amplihack-7");
    assert_eq!(item.source, "stewardship:rysweet/amplihack#7");
}
