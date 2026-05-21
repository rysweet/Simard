//! Post-cycle curation: promote backlog items and ingest meeting handoffs.

use crate::error::SimardResult;
use crate::goal_curation::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress};

/// Promote the highest-scoring backlog items into free active slots.
///
/// Backlog items are sorted by score descending and promoted until the
/// active board is at capacity or the backlog is empty.
pub fn promote_from_backlog(board: &mut GoalBoard) {
    // Sort backlog by score descending so we promote the best first.
    board.backlog.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    while board.active_slots_remaining() > 0 && !board.backlog.is_empty() {
        let item_id = board.backlog[0].id.clone();
        match crate::goal_curation::promote_to_active(board, &item_id, 3, None) {
            Ok(()) => {
                eprintln!("[simard] OODA curate: promoted backlog item '{item_id}' to active");
            }
            Err(e) => {
                eprintln!("[simard] OODA curate: failed to promote '{item_id}': {e}");
                break;
            }
        }
    }
}

/// Check for unprocessed meeting handoff artifacts in `handoff_dir`, convert
/// their decisions into active goals (or backlog items when at capacity) and
/// action items into backlog items on the board. Marks the handoff processed.
/// Returns the number of goals + backlog items created.
///
/// **FIFO ordering** (#1649): selects the **oldest** unprocessed handoff
/// among all candidates (lexicographic filename sort = chronological order
/// for `handoff-<rfc3339>.json`). The previous "newest by filename"
/// behaviour caused starvation: a fresh empty handoff (e.g. from a
/// dashboard chat closing with zero items) would permanently shadow an
/// older content-rich handoff because the older file was never selected
/// after a newer one had been marked processed.
pub fn check_meeting_handoffs(
    board: &mut GoalBoard,
    handoff_dir: &std::path::Path,
) -> SimardResult<u32> {
    use crate::meeting_facilitator::find_oldest_unprocessed_handoff;

    let path = match find_oldest_unprocessed_handoff(handoff_dir)? {
        Some(p) => p,
        None => return Ok(0),
    };

    // Diagnostic: surface starvation that would have occurred under the old
    // "newest by filename" selection — log when the oldest unprocessed file
    // is older than the newest file in the directory (meaning at least one
    // newer file exists and is already processed, or is itself unprocessed
    // but properly deferred).
    if let Some(newest) = crate::meeting_facilitator::find_newest_handoff(handoff_dir)
        && newest != path
    {
        tracing::info!(
            selected = %path.display(),
            newest = %newest.display(),
            "OODA curate: selecting older unprocessed handoff over newer file (FIFO)"
        );
    }

    let raw =
        std::fs::read_to_string(&path).map_err(|e| crate::error::SimardError::ArtifactIo {
            path: path.clone(),
            reason: format!("reading handoff: {e}"),
        })?;
    let mut handoff: crate::meeting_facilitator::MeetingHandoff = serde_json::from_str(&raw)
        .map_err(|e| crate::error::SimardError::ArtifactIo {
            path: path.clone(),
            reason: format!("failed to parse handoff JSON: {e}"),
        })?;

    let mut created = 0u32;

    // Convert decisions to active goals; overflow goes to backlog.
    for (i, decision) in handoff.decisions.iter().enumerate() {
        let goal_id = crate::goals::goal_slug(&decision.description);
        let description = format!("[meeting] {}", decision.description);

        // Deduplicate against existing active goals and backlog.
        if board.active.iter().any(|g| g.id == goal_id)
            || board.backlog.iter().any(|b| b.id == goal_id)
        {
            continue;
        }

        if board.active.len() < crate::goal_curation::MAX_ACTIVE_GOALS {
            // Priority based on position: earlier decisions = higher priority.
            let priority = (i as u32).saturating_add(1).min(5);
            board.active.push(ActiveGoal {
                id: goal_id,
                description,
                priority,
                status: GoalProgress::NotStarted,
                assigned_to: None,
                current_activity: None,
                wip_refs: vec![],
                last_progress_update_at: None,
            });
        } else {
            // Board full — route to backlog with score based on position.
            let score = 1.0 - (i as f64 * 0.1).min(0.9);
            board.backlog.push(BacklogItem {
                id: goal_id,
                description,
                source: format!("meeting:{}", handoff.topic),
                score,
            });
        }
        created += 1;
    }

    // Convert action items with priority >= 2 to backlog items.
    for item in &handoff.action_items {
        if item.priority < 2 {
            continue;
        }
        let item_id = crate::goals::goal_slug(&item.description);
        if board.backlog.iter().any(|b| b.id == item_id)
            || board.active.iter().any(|g| g.id == item_id)
        {
            continue;
        }
        // Higher action-item priority → higher backlog score.
        let score = (item.priority as f64 * 0.2).min(1.0);
        board.backlog.push(BacklogItem {
            id: item_id,
            description: format!("[action] {} (owner: {})", item.description, item.owner),
            source: format!("meeting:{}", handoff.topic),
            score,
        });
        created += 1;
    }

    // Mark this specific file processed and write back to the same path —
    // never let the path-lookup pick a sibling file (which is the core of
    // the #1649 starvation: under the old in_place helper the writeback
    // could land on the wrong file when multiple handoffs co-exist).
    handoff.processed = true;
    let json = serde_json::to_string_pretty(&handoff).map_err(|e| {
        crate::error::SimardError::ArtifactIo {
            path: path.clone(),
            reason: format!("serializing handoff: {e}"),
        }
    })?;
    std::fs::write(&path, &json).map_err(|e| crate::error::SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("writing handoff: {e}"),
    })?;

    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal_curation::{BacklogItem, GoalBoard, GoalProgress};
    use crate::meeting_facilitator::{
        ActionItem, MeetingDecision, MeetingHandoff, load_meeting_handoff, write_meeting_handoff,
    };
    use tempfile::TempDir;

    fn sample_handoff(decisions: Vec<MeetingDecision>) -> MeetingHandoff {
        MeetingHandoff {
            topic: "Sprint planning".to_string(),
            started_at: "2026-04-02T23:00:00Z".to_string(),
            closed_at: "2026-04-03T00:00:00Z".to_string(),
            decisions,
            action_items: Vec::new(),
            open_questions: Vec::new(),
            processed: false,
            duration_secs: None,
            transcript: Vec::new(),
            participants: Vec::new(),
            themes: Vec::new(),
            meeting_id: String::new(),
            transcript_path: None,
        }
    }

    fn sample_handoff_with_actions(
        decisions: Vec<MeetingDecision>,
        action_items: Vec<ActionItem>,
    ) -> MeetingHandoff {
        MeetingHandoff {
            topic: "Sprint planning".to_string(),
            started_at: "2026-04-02T23:00:00Z".to_string(),
            closed_at: "2026-04-03T00:00:00Z".to_string(),
            decisions,
            action_items,
            open_questions: Vec::new(),
            processed: false,
            duration_secs: None,
            transcript: Vec::new(),
            participants: Vec::new(),
            themes: Vec::new(),
            meeting_id: String::new(),
            transcript_path: None,
        }
    }

    fn sample_decision(desc: &str) -> MeetingDecision {
        MeetingDecision {
            description: desc.to_string(),
            rationale: format!("Rationale for {desc}"),
            participants: vec!["alice".to_string()],
        }
    }

    fn sample_action(desc: &str, owner: &str, priority: u32) -> ActionItem {
        ActionItem {
            description: desc.to_string(),
            owner: owner.to_string(),
            priority,
            due_description: None,
            linked_issue: None,
        }
    }

    #[test]
    fn check_meeting_handoffs_converts_decisions_to_goals() {
        let dir = TempDir::new().expect("create temp dir");
        let handoff = sample_handoff(vec![
            sample_decision("Migrate to async runtime"),
            sample_decision("Add integration tests"),
        ]);
        write_meeting_handoff(dir.path(), &handoff).expect("write test handoff");

        let mut board = GoalBoard::new();
        let count = check_meeting_handoffs(&mut board, dir.path())
            .expect("check_meeting_handoffs should succeed");

        assert_eq!(count, 2);
        assert_eq!(board.active.len(), 2);
        assert_eq!(
            board.active[0].description,
            "[meeting] Migrate to async runtime"
        );
        assert_eq!(
            board.active[1].description,
            "[meeting] Add integration tests"
        );
        assert!(matches!(board.active[0].status, GoalProgress::NotStarted));
    }

    #[test]
    fn check_meeting_handoffs_assigns_position_based_priority() {
        let dir = TempDir::new().expect("create temp dir");
        let handoff = sample_handoff(vec![
            sample_decision("First decision"),
            sample_decision("Second decision"),
            sample_decision("Third decision"),
        ]);
        write_meeting_handoff(dir.path(), &handoff).expect("write test handoff");

        let mut board = GoalBoard::new();
        check_meeting_handoffs(&mut board, dir.path())
            .expect("check_meeting_handoffs should succeed");

        assert_eq!(board.active[0].priority, 1);
        assert_eq!(board.active[1].priority, 2);
        assert_eq!(board.active[2].priority, 3);
    }

    #[test]
    fn check_meeting_handoffs_marks_handoff_processed() {
        let dir = TempDir::new().expect("create temp dir");
        let handoff = sample_handoff(vec![sample_decision("Ship v2")]);
        write_meeting_handoff(dir.path(), &handoff).expect("write test handoff");

        let mut board = GoalBoard::new();
        check_meeting_handoffs(&mut board, dir.path())
            .expect("check_meeting_handoffs should succeed");

        let reloaded = load_meeting_handoff(dir.path())
            .expect("load test handoff")
            .expect("handoff should exist");
        assert!(reloaded.processed);
    }

    #[test]
    fn check_meeting_handoffs_skips_already_processed() {
        let dir = TempDir::new().expect("create temp dir");
        let mut handoff = sample_handoff(vec![sample_decision("Already done")]);
        handoff.processed = true;
        write_meeting_handoff(dir.path(), &handoff).expect("write test handoff");

        let mut board = GoalBoard::new();
        let count = check_meeting_handoffs(&mut board, dir.path())
            .expect("check_meeting_handoffs should succeed");

        assert_eq!(count, 0);
        assert!(board.active.is_empty());
    }

    #[test]
    fn check_meeting_handoffs_no_file_returns_zero() {
        let dir = TempDir::new().expect("create temp dir");
        let mut board = GoalBoard::new();
        let count = check_meeting_handoffs(&mut board, dir.path())
            .expect("check_meeting_handoffs should succeed");
        assert_eq!(count, 0);
    }

    #[test]
    fn check_meeting_handoffs_overflow_goes_to_backlog() {
        let dir = TempDir::new().expect("create temp dir");
        // 7 decisions: 5 fit active, 2 overflow to backlog.
        let decisions: Vec<MeetingDecision> = (1..=7)
            .map(|i| sample_decision(&format!("Goal {i}")))
            .collect();
        write_meeting_handoff(dir.path(), &sample_handoff(decisions)).expect("write test handoff");

        let mut board = GoalBoard::new();
        let count = check_meeting_handoffs(&mut board, dir.path())
            .expect("check_meeting_handoffs should succeed");

        assert_eq!(count, 7);
        assert_eq!(board.active.len(), crate::goal_curation::MAX_ACTIVE_GOALS);
        assert_eq!(board.backlog.len(), 2);
        assert!(board.backlog[0].description.starts_with("[meeting]"));
        assert_eq!(board.backlog[0].source, "meeting:Sprint planning");
    }

    #[test]
    fn check_meeting_handoffs_skips_duplicate_goal_ids() {
        let dir = TempDir::new().expect("create temp dir");
        let handoff = sample_handoff(vec![
            sample_decision("Ship v2"),
            sample_decision("Ship v2"), // duplicate
        ]);
        write_meeting_handoff(dir.path(), &handoff).expect("write test handoff");

        let mut board = GoalBoard::new();
        check_meeting_handoffs(&mut board, dir.path())
            .expect("check_meeting_handoffs should succeed");

        assert_eq!(board.active.len(), 1);
    }

    #[test]
    fn check_meeting_handoffs_converts_action_items_to_backlog() {
        let dir = TempDir::new().expect("create temp dir");
        let handoff = sample_handoff_with_actions(
            vec![sample_decision("Main decision")],
            vec![
                sample_action("Write docs", "alice", 3), // priority >= 2 → backlog
                sample_action("Quick fix", "bob", 1),    // priority < 2 → skipped
                sample_action("Add metrics", "carol", 2), // priority >= 2 → backlog
            ],
        );
        write_meeting_handoff(dir.path(), &handoff).expect("write test handoff");

        let mut board = GoalBoard::new();
        let count = check_meeting_handoffs(&mut board, dir.path())
            .expect("check_meeting_handoffs should succeed");

        assert_eq!(count, 3); // 1 decision + 2 qualifying action items
        assert_eq!(board.active.len(), 1);
        assert_eq!(board.backlog.len(), 2);
        assert!(
            board.backlog[0]
                .description
                .contains("[action] Write docs (owner: alice)")
        );
        assert!(
            board.backlog[1]
                .description
                .contains("[action] Add metrics (owner: carol)")
        );
        assert_eq!(board.backlog[0].source, "meeting:Sprint planning");
    }

    // --- promote_from_backlog ---

    #[test]
    fn promote_from_backlog_fills_slots() {
        let mut board = GoalBoard::new();
        board.backlog.push(BacklogItem {
            id: "item-1".to_string(),
            description: "First".to_string(),
            source: "test".to_string(),
            score: 0.9,
        });
        board.backlog.push(BacklogItem {
            id: "item-2".to_string(),
            description: "Second".to_string(),
            source: "test".to_string(),
            score: 0.5,
        });
        promote_from_backlog(&mut board);
        assert!(board.active.len() <= crate::goal_curation::MAX_ACTIVE_GOALS);
        assert!(!board.active.is_empty());
    }

    #[test]
    fn promote_from_backlog_does_nothing_when_at_capacity() {
        let mut board = GoalBoard::new();
        for i in 0..crate::goal_curation::MAX_ACTIVE_GOALS {
            board.active.push(ActiveGoal {
                id: format!("g-{i}"),
                description: format!("Goal {i}"),
                priority: 1,
                status: GoalProgress::NotStarted,
                assigned_to: None,
                current_activity: None,
                wip_refs: vec![],
                last_progress_update_at: None,
            });
        }
        board.backlog.push(BacklogItem {
            id: "overflow".to_string(),
            description: "Overflow".to_string(),
            source: "test".to_string(),
            score: 0.9,
        });
        promote_from_backlog(&mut board);
        assert_eq!(board.active.len(), crate::goal_curation::MAX_ACTIVE_GOALS);
        assert_eq!(board.backlog.len(), 1, "backlog item should remain");
    }

    #[test]
    fn promote_from_backlog_empty_backlog() {
        let mut board = GoalBoard::new();
        promote_from_backlog(&mut board);
        assert!(board.active.is_empty());
    }

    // -----------------------------------------------------------------
    // FIFO regression test for #1649 (handoff starvation).
    //
    // Scenario: a content-rich handoff A is written first, then a fresh
    // empty handoff B is written. Under the old "newest by filename"
    // selection, B would be processed first (and A would be permanently
    // shadowed once B was marked processed). With FIFO-by-`created_at`
    // ascending among `pending` handoffs, A must be processed first.
    // -----------------------------------------------------------------
    #[test]
    fn check_meeting_handoffs_picks_oldest_unprocessed_first_fifo() {
        use std::fs;

        let dir = TempDir::new().expect("create temp dir");

        // Handoff A — older, content-rich.
        let mut handoff_a = sample_handoff(vec![sample_decision("Older meeting decision A")]);
        handoff_a.topic = "Older meeting".to_string();
        handoff_a.closed_at = "2026-04-03T00:00:00Z".to_string();
        let path_a = dir.path().join("handoff-2026-04-03T00-00-00_00-00.json");
        fs::write(&path_a, serde_json::to_string_pretty(&handoff_a).unwrap()).unwrap();

        // Handoff B — newer, empty (zero decisions, zero action items).
        let handoff_b = MeetingHandoff {
            topic: "Empty dashboard chat".to_string(),
            started_at: "2026-04-03T00:05:00Z".to_string(),
            closed_at: "2026-04-03T00:05:01Z".to_string(),
            decisions: Vec::new(),
            action_items: Vec::new(),
            open_questions: Vec::new(),
            processed: false,
            duration_secs: None,
            transcript: Vec::new(),
            participants: Vec::new(),
            themes: Vec::new(),
            meeting_id: String::new(),
            transcript_path: None,
        };
        let path_b = dir.path().join("handoff-2026-04-03T00-05-01_00-00.json");
        fs::write(&path_b, serde_json::to_string_pretty(&handoff_b).unwrap()).unwrap();

        let mut board = GoalBoard::new();

        // First cycle: must process A (older, content-rich) — NOT B.
        let count = check_meeting_handoffs(&mut board, dir.path())
            .expect("check_meeting_handoffs cycle 1 should succeed");
        assert_eq!(count, 1, "first cycle should ingest A's single decision");
        assert_eq!(board.active.len(), 1);
        assert_eq!(
            board.active[0].description, "[meeting] Older meeting decision A",
            "older handoff A must be processed first under FIFO ordering"
        );

        // A's file must be marked processed; B's must remain unprocessed.
        let reloaded_a: MeetingHandoff =
            serde_json::from_str(&fs::read_to_string(&path_a).unwrap()).unwrap();
        let reloaded_b: MeetingHandoff =
            serde_json::from_str(&fs::read_to_string(&path_b).unwrap()).unwrap();
        assert!(
            reloaded_a.processed,
            "handoff A must be marked processed after first cycle"
        );
        assert!(
            !reloaded_b.processed,
            "handoff B must remain unprocessed after first cycle"
        );

        // Second cycle: B is now the oldest unprocessed → gets processed
        // (with zero items, since it's empty). Demonstrates B is no longer
        // permanently shadowing A.
        let count2 = check_meeting_handoffs(&mut board, dir.path())
            .expect("check_meeting_handoffs cycle 2 should succeed");
        assert_eq!(
            count2, 0,
            "second cycle ingests empty handoff B → zero items"
        );
        let reloaded_b2: MeetingHandoff =
            serde_json::from_str(&fs::read_to_string(&path_b).unwrap()).unwrap();
        assert!(
            reloaded_b2.processed,
            "handoff B must be marked processed after second cycle"
        );

        // Third cycle: nothing left to process.
        let count3 = check_meeting_handoffs(&mut board, dir.path())
            .expect("check_meeting_handoffs cycle 3 should succeed");
        assert_eq!(count3, 0, "no unprocessed handoffs remain");
    }
}
