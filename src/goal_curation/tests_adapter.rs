//! Failing TDD tests (issue #1590, Step 7) for the goal-board → flat-record
//! adapter required by the engineer / meeting code paths.
//!
//! Spec section A3 — `pub fn active_goals_as_records(&GoalBoard) -> Vec<GoalRecord>`
//! lives in `src/goal_curation/operations.rs` and is re-exported from
//! `src/goal_curation/mod.rs`. Mapping:
//!
//! | Field                | Source                                                      |
//! |----------------------|-------------------------------------------------------------|
//! | `slug`               | `goal_slug(active.id)` (or `active.id` if already a slug)   |
//! | `title`              | `active.description` (first line, truncated to 120 chars)   |
//! | `rationale`          | `active.current_activity.unwrap_or_default()`               |
//! | `status`             | `Completed → GoalStatus::Completed`, else `GoalStatus::Active` |
//! | `priority`           | `u8::try_from(active.priority).unwrap_or(u8::MAX)`          |
//! | `owner_identity`     | `active.assigned_to.clone().unwrap_or_else(\|\| "unassigned".into())` |
//! | `source_session_id`  | sentinel `00000000-0000-0000-0000-000000000000`             |
//! | `updated_in`         | `SessionPhase::Persistence`                                 |
//!
//! These tests will not compile until `active_goals_as_records` is added and
//! re-exported. That is the intended "red" state for TDD.

use super::operations::active_goals_as_records;
use super::types::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, WipRef};

use crate::goals::GoalStatus;

fn active(id: &str, description: &str, priority: u32) -> ActiveGoal {
    ActiveGoal {
        id: id.to_string(),
        description: description.to_string(),
        priority,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    }
}

#[test]
fn empty_board_yields_no_records() {
    let board = GoalBoard::new();
    assert!(active_goals_as_records(&board).is_empty());
}

#[test]
fn backlog_items_are_not_emitted_as_records() {
    let mut board = GoalBoard::new();
    board.backlog.push(BacklogItem {
        id: "backlog-only".to_string(),
        description: "Should never surface as a GoalRecord".to_string(),
        source: "test".to_string(),
        score: 0.5,
    });

    let records = active_goals_as_records(&board);
    assert!(
        records.is_empty(),
        "active_goals_as_records must only emit active goals, got {} records",
        records.len()
    );
}

#[test]
fn one_active_goal_maps_basic_fields() {
    let mut board = GoalBoard::new();
    board.active.push(active(
        "issue-1590-migrate-goal-records",
        "Migrate all consumers of goal_records.json to cognitive memory",
        2,
    ));

    let records = active_goals_as_records(&board);
    assert_eq!(records.len(), 1);
    let r = &records[0];

    assert_eq!(r.slug, "issue-1590-migrate-goal-records");
    assert_eq!(
        r.title,
        "Migrate all consumers of goal_records.json to cognitive memory"
    );
    assert_eq!(r.priority, 2);
    assert_eq!(
        r.status,
        GoalStatus::Active,
        "non-completed goals must map to Active so the engineer / meeting paths still see them"
    );
}

#[test]
fn current_activity_is_used_as_rationale_when_present() {
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "improve-coverage".to_string(),
        description: "Improve test coverage on the goal curation module".to_string(),
        priority: 3,
        status: GoalProgress::InProgress { percent: 25 },
        assigned_to: None,
        current_activity: Some("writing tests for active_goals_as_records".to_string()),
        wip_refs: vec![],
        last_progress_update_at: None,
    });

    let records = active_goals_as_records(&board);
    assert_eq!(
        records[0].rationale,
        "writing tests for active_goals_as_records"
    );
}

#[test]
fn missing_current_activity_yields_empty_rationale() {
    let mut board = GoalBoard::new();
    board
        .active
        .push(active("no-activity-yet", "Bootstrap a brand new goal", 1));

    let records = active_goals_as_records(&board);
    assert_eq!(records[0].rationale, "");
}

#[test]
fn assigned_to_some_becomes_owner_identity() {
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "assigned-goal-id".to_string(),
        description: "An assigned goal".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: Some("simard-engineer".to_string()),
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });

    let records = active_goals_as_records(&board);
    assert_eq!(records[0].owner_identity, "simard-engineer");
}

#[test]
fn unassigned_goal_uses_unassigned_sentinel_for_owner() {
    let mut board = GoalBoard::new();
    board
        .active
        .push(active("unassigned-goal-id", "Nobody owns this yet", 1));

    let records = active_goals_as_records(&board);
    assert_eq!(records[0].owner_identity, "unassigned");
}

#[test]
fn priority_above_u8_max_saturates_at_u8_max() {
    let mut board = GoalBoard::new();
    let mut g = active("huge-priority-goal", "Priority overflows u8", 999_999);
    g.assigned_to = Some("ops".to_string());
    board.active.push(g);

    let records = active_goals_as_records(&board);
    assert_eq!(
        records[0].priority,
        u8::MAX,
        "priority overflow must saturate at u8::MAX rather than panic"
    );
}

#[test]
fn status_completed_maps_to_completed() {
    let mut board = GoalBoard::new();
    let mut g = active("done-goal-id", "Already shipped", 1);
    g.status = GoalProgress::Completed;
    board.active.push(g);

    let records = active_goals_as_records(&board);
    assert_eq!(records[0].status, GoalStatus::Completed);
}

#[test]
fn status_in_progress_blocked_and_not_started_all_map_to_active() {
    let mut board = GoalBoard::new();
    board.active.push({
        let mut g = active("in-progress-goal-id", "Working on it", 1);
        g.status = GoalProgress::InProgress { percent: 50 };
        g
    });
    board.active.push({
        let mut g = active("blocked-goal-id", "Stuck behind dependency", 2);
        g.status = GoalProgress::Blocked("waiting on review".to_string());
        g
    });
    board.active.push({
        let mut g = active("not-started-goal-id", "Queued for engineer pickup", 3);
        g.status = GoalProgress::NotStarted;
        g
    });

    let records = active_goals_as_records(&board);
    assert_eq!(records.len(), 3);
    for r in &records {
        assert_eq!(
            r.status,
            GoalStatus::Active,
            "non-completed goals must map to Active so engineer dispatch keeps seeing them; got {:?}",
            r.status
        );
    }
}

#[test]
fn long_description_is_truncated_to_120_chars_for_title() {
    let long = "x".repeat(200);
    let mut board = GoalBoard::new();
    board.active.push(active("long-desc-goal-id", &long, 1));

    let records = active_goals_as_records(&board);
    assert!(
        records[0].title.chars().count() <= 120,
        "title must be truncated to <=120 chars, got {} chars",
        records[0].title.chars().count()
    );
}

#[test]
fn description_first_line_only_is_used_for_title() {
    let mut board = GoalBoard::new();
    board.active.push(active(
        "multi-line-goal-id",
        "First line summary\nDetailed paragraph\nMore detail",
        1,
    ));

    let records = active_goals_as_records(&board);
    assert_eq!(
        records[0].title, "First line summary",
        "only the first line of description should land in title"
    );
}

#[test]
fn ids_already_in_slug_form_are_preserved() {
    // Slug form per goal_slug(): lowercase ASCII alphanumeric + dashes.
    let mut board = GoalBoard::new();
    board
        .active
        .push(active("self-improvement", "Self-improvement work", 1));

    let records = active_goals_as_records(&board);
    assert_eq!(records[0].slug, "self-improvement");
}

#[test]
fn ids_with_uppercase_or_punctuation_are_slugified() {
    let mut board = GoalBoard::new();
    board.active.push(active(
        "Goal With Uppercase! And Punctuation",
        "Slugify me",
        1,
    ));

    let records = active_goals_as_records(&board);
    assert_eq!(
        records[0].slug, "goal-with-uppercase-and-punctuation",
        "non-slug ids must be normalised through goal_slug()"
    );
}

#[test]
fn ordering_of_records_matches_ordering_of_active_goals() {
    let mut board = GoalBoard::new();
    board.active.push(active("first-goal-id", "First", 1));
    board.active.push(active("second-goal-id", "Second", 2));
    board.active.push(active("third-goal-id", "Third", 3));

    let records = active_goals_as_records(&board);
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].slug, "first-goal-id");
    assert_eq!(records[1].slug, "second-goal-id");
    assert_eq!(records[2].slug, "third-goal-id");
}

#[test]
fn wip_refs_do_not_affect_record_construction() {
    let mut board = GoalBoard::new();
    let mut g = active("with-wip-goal-id", "Has WIP refs attached", 1);
    g.wip_refs.push(WipRef {
        kind: "pr".to_string(),
        ref_id: "1234".to_string(),
        label: "PR #1234".to_string(),
        url: None,
    });
    board.active.push(g);

    let records = active_goals_as_records(&board);
    assert_eq!(records.len(), 1);
    // WIP refs are not part of GoalRecord — adapter must not panic on them.
    assert_eq!(records[0].slug, "with-wip-goal-id");
}
