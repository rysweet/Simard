//! Failing TDD tests (issue #2922, Step 7) for the reverse goal adapter
//! `record_as_active_goal` — the inverse of [`super::operations::active_goals_as_records`].
//!
//! The live goal-board read (`dashboard_live_goal_board`, #2922) unions the
//! stale `goal-board:snapshot` board with a LIVE `CognitiveMemoryGoalStore`
//! overlay of `goal-store:record` facts (creative-idea Proposed goals, meeting
//! goals, …). To render an overlay record on the board it must be mapped BACK
//! into the board's `ActiveGoal` / `BacklogItem` shapes. That mapping is
//! `record_as_active_goal`, which places each record into one board bucket:
//!
//! ```ignore
//! #[derive(Debug)]
//! pub enum BoardPlacement { Active(ActiveGoal), Backlog(BacklogItem), Skip }
//! pub fn record_as_active_goal(record: &GoalRecord) -> BoardPlacement;
//! ```
//!
//! The enum is unboxed and derives `Debug` (these tests move the variant payload
//! out and `{:?}`-format it). If clippy flags `large_enum_variant`, the
//! implementation may add `#[allow(clippy::large_enum_variant)]` rather than
//! boxing — boxing is not part of this contract.
//!
//! Status routing (mirrors the board's active/backlog split):
//!
//! | `GoalRecord.status` | Placement  | Rendered progress          |
//! |---------------------|------------|----------------------------|
//! | `Active`            | `active[]` | `InProgress { percent: 0 }`|
//! | `Proposed`          | `backlog[]`| — (backlog item)           |
//! | `Paused`            | `backlog[]`| —                          |
//! | `Completed`         | **Skip**   | terminal; not surfaced     |
//!
//! These tests reference `record_as_active_goal` and `BoardPlacement`, which do
//! not exist yet — that is the intended TDD red (compile-fail) state, exactly as
//! the sibling `tests_adapter.rs` did for the forward adapter.

use super::labels::SOURCE_CREATIVE_IDEAS;
use super::operations::{BoardPlacement, record_as_active_goal};
use super::types::{ActiveGoal, BacklogItem, GoalProgress};

use crate::goals::{GoalRecord, GoalStatus, goal_slug};
use crate::session::{SessionId, SessionPhase};

/// Build a `GoalRecord` with the given status/priority/labels. `slug` is derived
/// from `title` via `goal_slug`, mirroring how the creative-ideas router
/// (`route_idea_to_goal`) constructs the record it `put`s.
fn record(title: &str, status: GoalStatus, priority: u8, labels: Vec<String>) -> GoalRecord {
    GoalRecord {
        wip_refs: Vec::new(),
        slug: goal_slug(title),
        title: title.to_string(),
        rationale: "why this goal matters".to_string(),
        status,
        priority,
        owner_identity: "creative-ideas".to_string(),
        source_session_id: SessionId::parse("session-018f1f7e-4c5d-7b2a-8f10-b5c0d4f7b123")
            .expect("session id should parse"),
        updated_in: SessionPhase::Planning,
        evidence: Vec::new(),
        labels,
    }
}

/// Extract the `ActiveGoal` from an `Active` placement, or panic with context.
fn expect_active(placement: BoardPlacement) -> ActiveGoal {
    match placement {
        BoardPlacement::Active(goal) => goal,
        other => panic!("expected BoardPlacement::Active, got {other:?}"),
    }
}

/// Extract the `BacklogItem` from a `Backlog` placement, or panic with context.
fn expect_backlog(placement: BoardPlacement) -> BacklogItem {
    match placement {
        BoardPlacement::Backlog(item) => item,
        other => panic!("expected BoardPlacement::Backlog, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Status routing
// ---------------------------------------------------------------------------

#[test]
fn active_record_routes_to_active_bucket() {
    let placement = record_as_active_goal(&record(
        "Drive amplihack-rs to feature parity",
        GoalStatus::Active,
        2,
        Vec::new(),
    ));
    assert!(
        matches!(placement, BoardPlacement::Active(_)),
        "an Active goal-store record must render on the active board"
    );
}

#[test]
fn proposed_record_routes_to_backlog_bucket() {
    // The primary #2922 case: a promoted creative idea persists a Proposed
    // record, which must show up on the Goals tab's proposed backlog.
    let placement = record_as_active_goal(&record(
        "Improve recall precision",
        GoalStatus::Proposed,
        3,
        vec![SOURCE_CREATIVE_IDEAS.to_string()],
    ));
    assert!(
        matches!(placement, BoardPlacement::Backlog(_)),
        "a Proposed goal-store record must render as a backlog item (issue #2922)"
    );
}

#[test]
fn paused_record_routes_to_backlog_bucket() {
    let placement = record_as_active_goal(&record(
        "Deferred cleanup pass",
        GoalStatus::Paused,
        4,
        Vec::new(),
    ));
    assert!(
        matches!(placement, BoardPlacement::Backlog(_)),
        "a Paused goal-store record must render as a backlog item"
    );
}

#[test]
fn completed_record_is_skipped() {
    let placement = record_as_active_goal(&record(
        "Already shipped work",
        GoalStatus::Completed,
        1,
        Vec::new(),
    ));
    assert!(
        matches!(placement, BoardPlacement::Skip),
        "a Completed record is terminal and must NOT be surfaced on the live board"
    );
}

// ---------------------------------------------------------------------------
// Active-bucket field synthesis
// ---------------------------------------------------------------------------

#[test]
fn active_record_maps_id_description_and_priority() {
    let rec = record(
        "Ship the fail-closed live read",
        GoalStatus::Active,
        2,
        Vec::new(),
    );
    let goal = expect_active(record_as_active_goal(&rec));

    assert_eq!(goal.id, rec.slug, "ActiveGoal.id must be the record slug");
    assert_eq!(
        goal.description, rec.title,
        "ActiveGoal.description must be the record title"
    );
    assert_eq!(
        goal.priority, rec.priority as u32,
        "record priority (u8) must widen to ActiveGoal.priority (u32)"
    );
}

#[test]
fn active_record_status_is_in_progress_zero_percent() {
    let goal = expect_active(record_as_active_goal(&record(
        "Live board goal",
        GoalStatus::Active,
        1,
        Vec::new(),
    )));
    assert_eq!(
        goal.status,
        GoalProgress::InProgress { percent: 0 },
        "an Active record has no persisted percent, so it renders as InProgress {{ percent: 0 }}"
    );
}

#[test]
fn active_record_owner_becomes_assigned_to() {
    let mut rec = record("Owned goal", GoalStatus::Active, 1, Vec::new());
    rec.owner_identity = "simard-engineer".to_string();
    let goal = expect_active(record_as_active_goal(&rec));
    assert_eq!(
        goal.assigned_to.as_deref(),
        Some("simard-engineer"),
        "record.owner_identity must map to ActiveGoal.assigned_to"
    );
}

#[test]
fn active_record_unassigned_owner_maps_to_none() {
    let mut rec = record("Unowned goal", GoalStatus::Active, 1, Vec::new());
    rec.owner_identity = "unassigned".to_string();
    let goal = expect_active(record_as_active_goal(&rec));
    assert_eq!(
        goal.assigned_to, None,
        "the 'unassigned' sentinel owner must map back to assigned_to = None"
    );
}

#[test]
fn active_record_preserves_labels() {
    let rec = record(
        "Creative-idea goal",
        GoalStatus::Active,
        3,
        vec![SOURCE_CREATIVE_IDEAS.to_string()],
    );
    let goal = expect_active(record_as_active_goal(&rec));
    assert!(
        goal.labels.contains(&SOURCE_CREATIVE_IDEAS.to_string()),
        "provenance labels must carry through the reverse adapter; got {:?}",
        goal.labels
    );
}

#[test]
fn active_record_synthesizes_absent_rich_fields() {
    // A GoalRecord carries none of the snapshot-only rich fields, so the adapter
    // must synthesize them so the JSON stays byte-stable.
    let goal = expect_active(record_as_active_goal(&record(
        "Minimal record",
        GoalStatus::Active,
        1,
        Vec::new(),
    )));
    assert_eq!(goal.repo, None, "repo must be synthesized as None");
    assert_eq!(
        goal.current_activity, None,
        "current_activity must be synthesized as None"
    );
    assert_eq!(
        goal.parent_goal_id, None,
        "parent_goal_id must be synthesized as None"
    );
    assert!(
        goal.wip_refs.is_empty(),
        "wip_refs must be synthesized as []"
    );
    assert!(
        !goal.priority_explicit,
        "priority_explicit must be synthesized as false (record has no such provenance)"
    );
    assert_eq!(
        goal.last_progress_update_at, None,
        "last_progress_update_at must be synthesized as None"
    );
}

// ---------------------------------------------------------------------------
// Backlog-bucket field synthesis
// ---------------------------------------------------------------------------

#[test]
fn proposed_record_maps_id_and_description() {
    let rec = record("Proposed backlog goal", GoalStatus::Proposed, 3, Vec::new());
    let item = expect_backlog(record_as_active_goal(&rec));
    assert_eq!(item.id, rec.slug, "BacklogItem.id must be the record slug");
    assert_eq!(
        item.description, rec.title,
        "BacklogItem.description must be the record title"
    );
}

#[test]
fn proposed_creative_idea_record_has_plain_english_source() {
    let rec = record(
        "Proposed from a creative idea",
        GoalStatus::Proposed,
        3,
        vec![SOURCE_CREATIVE_IDEAS.to_string()],
    );
    let item = expect_backlog(record_as_active_goal(&rec));
    assert!(
        item.source.to_lowercase().contains("creative"),
        "a source:creative-ideas record must get a plain-English provenance label \
         mentioning creative ideas, not the raw label; got {:?}",
        item.source
    );
    assert!(
        !item.source.contains("source:"),
        "the raw `source:*` label must not leak into the backlog source; got {:?}",
        item.source
    );
}

#[test]
fn backlog_score_is_finite_and_ranks_higher_priority_first() {
    // "Higher priority" in this codebase means a LOWER priority number (p1 is
    // most important). The doc mandates higher priority -> higher score so the
    // proposed backlog orders deterministically.
    let high = expect_backlog(record_as_active_goal(&record(
        "Most important proposal",
        GoalStatus::Proposed,
        1,
        Vec::new(),
    )));
    let low = expect_backlog(record_as_active_goal(&record(
        "Least important proposal",
        GoalStatus::Proposed,
        9,
        Vec::new(),
    )));

    assert!(
        high.score.is_finite(),
        "score must be finite, got {}",
        high.score
    );
    assert!(
        low.score.is_finite(),
        "score must be finite, got {}",
        low.score
    );
    assert!(
        high.score > low.score,
        "a higher-priority (p1) proposal must score above a lower-priority (p9) one \
         so backlog ordering is stable; got p1={} vs p9={}",
        high.score,
        low.score
    );
}

// ---------------------------------------------------------------------------
// Robustness — panic-free on arbitrary record text
// ---------------------------------------------------------------------------

#[test]
fn adapter_is_panic_free_on_arbitrary_text() {
    // Overlay records carry untrusted, model-generated text. The adapter is a
    // pure struct mapping and must never panic (a panic in the read path would
    // 500 / DoS the dashboard).
    let weird_titles = [
        "",
        "   ",
        "🦀 unicode goal with emoji 🚀 and\nnewlines\tand tabs",
        &"x".repeat(10_000),
        "slug/with:colons and spaces & Punctuation!!!",
    ];
    for title in weird_titles {
        for status in [
            GoalStatus::Active,
            GoalStatus::Proposed,
            GoalStatus::Paused,
            GoalStatus::Completed,
        ] {
            let mut rec = record("placeholder", status, 1, Vec::new());
            rec.title = title.to_string();
            rec.slug = title.to_string();
            rec.owner_identity = title.to_string();
            // Must not panic for any (title, status) combination.
            let _ = record_as_active_goal(&rec);
        }
    }
}

#[test]
fn priority_at_u8_max_widens_without_panic() {
    let rec = record(
        "Max priority proposal",
        GoalStatus::Active,
        u8::MAX,
        Vec::new(),
    );
    let goal = expect_active(record_as_active_goal(&rec));
    assert_eq!(
        goal.priority,
        u32::from(u8::MAX),
        "u8::MAX priority must widen cleanly to u32 without panic or truncation"
    );
}
