//! Post-cycle curation: promote backlog items and ingest meeting handoffs.

use std::collections::HashSet;
use std::path::Path;

use crate::error::SimardResult;
use crate::goal_curation::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress};

/// Path to the goal tombstone file relative to state root.
const TOMBSTONE_FILENAME: &str = "goal_tombstones.json";

/// Load the set of tombstoned goal IDs from disk.
/// Returns an empty set if the file doesn't exist or can't be parsed.
pub fn load_tombstones(state_root: &Path) -> HashSet<String> {
    let path = state_root.join(TOMBSTONE_FILENAME);
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => HashSet::new(),
    }
}

/// Save the tombstone set to disk.
pub fn save_tombstones(state_root: &Path, tombstones: &HashSet<String>) -> SimardResult<()> {
    let path = state_root.join(TOMBSTONE_FILENAME);
    let json = serde_json::to_string_pretty(tombstones).map_err(|e| {
        crate::error::SimardError::ArtifactIo {
            path: path.clone(),
            reason: format!("serializing tombstones: {e}"),
        }
    })?;
    std::fs::write(&path, &json).map_err(|e| crate::error::SimardError::ArtifactIo {
        path: path.clone(),
        reason: format!("writing tombstones: {e}"),
    })?;
    Ok(())
}

/// Record goal IDs as tombstoned so they won't be re-ingested from meeting handoffs.
pub fn tombstone_goals(state_root: &Path, ids: &[String]) -> SimardResult<()> {
    let mut tombstones = load_tombstones(state_root);
    for id in ids {
        tombstones.insert(id.clone());
    }
    save_tombstones(state_root, &tombstones)
}

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

/// Maximum number of handoffs processed per OODA cycle. Prevents resource
/// exhaustion if the handoff directory is flooded with files.
const BATCH_HANDOFF_LIMIT: usize = 10;

/// Check for unprocessed meeting handoff artifacts in `handoff_dir`, convert
/// their decisions into active goals (or backlog items when at capacity) and
/// action items into backlog items on the board. Marks the handoff processed.
/// Returns the number of goals + backlog items created.
///
/// **Batch processing** (#2268): processes up to [`BATCH_HANDOFF_LIMIT`]
/// unprocessed handoffs per cycle (FIFO order). Empty handoffs (0 decisions
/// AND 0 action_items) are fast-marked as processed without incrementing
/// the created count.
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
    state_root: &Path,
) -> SimardResult<u32> {
    use crate::meeting_facilitator::find_oldest_unprocessed_handoff;

    let tombstones = load_tombstones(state_root);
    let mut created = 0u32;

    for batch_idx in 0..BATCH_HANDOFF_LIMIT {
        let path = match find_oldest_unprocessed_handoff(handoff_dir)? {
            Some(p) => p,
            None => break,
        };

        // FIFO diagnostic (first iteration only to avoid log spam).
        if batch_idx == 0
            && let Some(newest) = crate::meeting_facilitator::find_newest_handoff(handoff_dir)
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

        // Fast-mark empty handoffs: 0 decisions AND 0 action_items means
        // there is nothing to curate — mark processed and move to the next
        // without incrementing `created`.
        if handoff.decisions.is_empty() && handoff.action_items.is_empty() {
            tracing::info!(
                path = %path.display(),
                "OODA curate: fast-marking empty handoff as processed"
            );
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
            continue;
        }

        let owner_hint = handoff.next_owner.as_deref();
        let artifact_suffix: String = if handoff.artifacts.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = handoff
                .artifacts
                .iter()
                .map(|a| format!("{}={}", a.kind, a.uri_or_path))
                .collect();
            format!(" artifacts=[{}]", names.join("; "))
        };

        // Convert decisions to active goals; overflow goes to backlog.
        for (i, decision) in handoff.decisions.iter().enumerate() {
            let goal_id = crate::goals::goal_slug(&decision.description);
            let description = format!("[meeting] {}", decision.description);

            if board.active.iter().any(|g| g.id == goal_id)
                || board.backlog.iter().any(|b| b.id == goal_id)
                || tombstones.contains(&goal_id)
            {
                continue;
            }

            if board.active.len() < crate::goal_curation::MAX_ACTIVE_GOALS {
                let priority = (i as u32).saturating_add(1).min(5);
                board.active.push(ActiveGoal {
                    id: goal_id,
                    description,
                    priority,
                    status: GoalProgress::NotStarted,
                    assigned_to: owner_hint.map(String::from),
                    current_activity: None,
                    wip_refs: vec![],
                    last_progress_update_at: None,
                });
            } else {
                let score = 1.0 - (i as f64 * 0.1).min(0.9);
                board.backlog.push(BacklogItem {
                    id: goal_id,
                    description,
                    source: format!(
                        "meeting:{}{}{}",
                        handoff.topic,
                        owner_hint
                            .map(|o| format!(" owner={o}"))
                            .unwrap_or_default(),
                        artifact_suffix,
                    ),
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
                || tombstones.contains(&item_id)
            {
                continue;
            }
            let score = (item.priority as f64 * 0.2).min(1.0);
            board.backlog.push(BacklogItem {
                id: item_id,
                description: format!("[action] {} (owner: {})", item.description, item.owner),
                source: format!(
                    "meeting:{}{}{}",
                    handoff.topic,
                    owner_hint
                        .map(|o| format!(" owner={o}"))
                        .unwrap_or_default(),
                    artifact_suffix,
                ),
                score,
            });
            created += 1;
        }

        // Mark processed and write back to the same path.
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
    }

    Ok(created)
}

/// Delete processed handoff JSON files older than 7 days to prevent
/// indefinite disk accumulation. Returns the count of files deleted.
///
/// Uses `symlink_metadata` (not `metadata`) to prevent symlink-following
/// attacks. Requires BOTH `processed == true` (parsed from JSON) AND
/// file mtime > 7 days before deletion — either condition alone is
/// insufficient. Per-file errors are logged and skipped (non-fatal).
///
/// Filename filter: `starts_with("handoff-") && ends_with(".json")`.
pub fn reap_old_handoffs(handoff_dir: &std::path::Path) -> SimardResult<u32> {
    use std::time::{Duration, SystemTime};

    const REAP_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

    let entries = match std::fs::read_dir(handoff_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(crate::error::SimardError::ArtifactIo {
                path: handoff_dir.to_path_buf(),
                reason: format!("reading handoff dir for reap: {e}"),
            });
        }
    };

    let cutoff = SystemTime::now() - REAP_AGE;
    let mut deleted = 0u32;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("reap_old_handoffs: skipping unreadable dir entry: {e}");
                continue;
            }
        };

        let fname = entry.file_name();
        let fname_str = fname.to_string_lossy();
        if !fname_str.starts_with("handoff-") || !fname_str.ends_with(".json") {
            continue;
        }

        let path = entry.path();

        // Use symlink_metadata to prevent symlink-following attacks.
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(path = %path.display(), "reap_old_handoffs: cannot stat: {e}");
                continue;
            }
        };

        // Skip if not a regular file (e.g. symlink, directory).
        if !meta.is_file() {
            continue;
        }

        let mtime = match meta.modified() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(path = %path.display(), "reap_old_handoffs: cannot read mtime: {e}");
                continue;
            }
        };

        if mtime > cutoff {
            continue; // Too recent.
        }

        // Parse JSON to verify processed == true before deleting.
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %path.display(), "reap_old_handoffs: cannot read: {e}");
                continue;
            }
        };
        let handoff: crate::meeting_facilitator::MeetingHandoff = match serde_json::from_str(&raw) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(path = %path.display(), "reap_old_handoffs: cannot parse: {e}");
                continue;
            }
        };

        if !handoff.processed {
            continue; // Not yet consumed — preserve.
        }

        // Both conditions met: processed AND older than 7 days — delete.
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!(path = %path.display(), "reap_old_handoffs: deleted old processed handoff");
                deleted += 1;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // TOCTOU race — file vanished between stat and remove.
                continue;
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), "reap_old_handoffs: failed to delete: {e}");
            }
        }
    }

    Ok(deleted)
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
            schema_version: 2,
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
            next_owner: None,
            artifacts: Vec::new(),
            goal: None,
            next_actor: None,
            applied_templates: Vec::new(),
            history_truncated_count: 0,
            partial_reason: None,
            risks: vec![],
            disagreements: vec![],
        }
    }

    fn sample_handoff_with_actions(
        decisions: Vec<MeetingDecision>,
        action_items: Vec<ActionItem>,
    ) -> MeetingHandoff {
        MeetingHandoff {
            schema_version: 2,
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
            next_owner: None,
            artifacts: Vec::new(),
            goal: None,
            next_actor: None,
            applied_templates: Vec::new(),
            history_truncated_count: 0,
            partial_reason: None,
            risks: vec![],
            disagreements: vec![],
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
        let count = check_meeting_handoffs(&mut board, dir.path(), dir.path())
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
        check_meeting_handoffs(&mut board, dir.path(), dir.path())
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
        check_meeting_handoffs(&mut board, dir.path(), dir.path())
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
        let count = check_meeting_handoffs(&mut board, dir.path(), dir.path())
            .expect("check_meeting_handoffs should succeed");

        assert_eq!(count, 0);
        assert!(board.active.is_empty());
    }

    #[test]
    fn check_meeting_handoffs_no_file_returns_zero() {
        let dir = TempDir::new().expect("create temp dir");
        let mut board = GoalBoard::new();
        let count = check_meeting_handoffs(&mut board, dir.path(), dir.path())
            .expect("check_meeting_handoffs should succeed");
        assert_eq!(count, 0);
    }

    #[test]
    fn check_meeting_handoffs_overflow_goes_to_backlog() {
        let dir = TempDir::new().expect("create temp dir");
        // MAX+2 decisions: MAX fit active, 2 overflow to backlog.
        let decisions: Vec<MeetingDecision> = (1..=(crate::goal_curation::MAX_ACTIVE_GOALS + 2))
            .map(|i| sample_decision(&format!("Goal {i}")))
            .collect();
        write_meeting_handoff(dir.path(), &sample_handoff(decisions)).expect("write test handoff");

        let mut board = GoalBoard::new();
        let count = check_meeting_handoffs(&mut board, dir.path(), dir.path())
            .expect("check_meeting_handoffs should succeed");

        assert_eq!(count, (crate::goal_curation::MAX_ACTIVE_GOALS + 2) as u32);
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
        check_meeting_handoffs(&mut board, dir.path(), dir.path())
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
        let count = check_meeting_handoffs(&mut board, dir.path(), dir.path())
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
    // FIFO regression test for #1649 (handoff starvation), updated for
    // #2268 batch processing.
    //
    // Scenario: a content-rich handoff A is written first, then a fresh
    // empty handoff B is written. With batch processing (up to 10 per
    // cycle) and FIFO ordering, cycle 1 processes BOTH: A first
    // (content-rich → created=1) then B (empty → fast-marked, created=0).
    // Cycle 2 finds nothing.
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
            schema_version: 2,
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
            next_owner: None,
            artifacts: Vec::new(),
            goal: None,
            next_actor: None,
            applied_templates: Vec::new(),
            history_truncated_count: 0,
            partial_reason: None,
            risks: vec![],
            disagreements: vec![],
        };
        let path_b = dir.path().join("handoff-2026-04-03T00-05-01_00-00.json");
        fs::write(&path_b, serde_json::to_string_pretty(&handoff_b).unwrap()).unwrap();

        let mut board = GoalBoard::new();

        // Cycle 1 (batch): processes both A (1 created) and B (fast-marked, 0 created).
        let count = check_meeting_handoffs(&mut board, dir.path(), dir.path())
            .expect("check_meeting_handoffs cycle 1 should succeed");
        assert_eq!(
            count, 1,
            "cycle 1 should ingest A's decision and fast-mark B"
        );
        assert_eq!(board.active.len(), 1);
        assert_eq!(
            board.active[0].description, "[meeting] Older meeting decision A",
            "older handoff A must be processed first under FIFO ordering"
        );

        // Both A and B must be marked processed after the batch cycle.
        let reloaded_a: MeetingHandoff =
            serde_json::from_str(&fs::read_to_string(&path_a).unwrap()).unwrap();
        let reloaded_b: MeetingHandoff =
            serde_json::from_str(&fs::read_to_string(&path_b).unwrap()).unwrap();
        assert!(
            reloaded_a.processed,
            "handoff A must be marked processed after batch cycle"
        );
        assert!(
            reloaded_b.processed,
            "handoff B must be fast-marked processed after batch cycle"
        );

        // Cycle 2: nothing left to process.
        let count2 = check_meeting_handoffs(&mut board, dir.path(), dir.path())
            .expect("check_meeting_handoffs cycle 2 should succeed");
        assert_eq!(
            count2, 0,
            "no unprocessed handoffs remain after batch cycle"
        );
    }

    // -----------------------------------------------------------------
    // Batch processing tests for #2268.
    // -----------------------------------------------------------------

    #[test]
    fn batch_processes_multiple_handoffs() {
        use std::fs;

        let dir = TempDir::new().expect("create temp dir");

        // Create 3 content-rich handoffs with ascending timestamps.
        for i in 0..3u8 {
            let handoff =
                sample_handoff(vec![sample_decision(&format!("Decision from meeting {i}"))]);
            let ts = format!("handoff-2026-05-0{}T00-00-00_00-00.json", i + 1);
            fs::write(
                dir.path().join(&ts),
                serde_json::to_string_pretty(&handoff).unwrap(),
            )
            .unwrap();
        }

        let mut board = GoalBoard::new();
        let count = check_meeting_handoffs(&mut board, dir.path(), dir.path())
            .expect("batch processing should succeed");

        // All 3 handoffs should be processed in a single call.
        assert_eq!(count, 3, "batch should process all 3 handoffs");
        assert_eq!(board.active.len(), 3);

        // Verify all files are marked processed.
        for entry in fs::read_dir(dir.path()).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name().to_string_lossy().starts_with("handoff-") {
                let raw = fs::read_to_string(entry.path()).unwrap();
                let h: MeetingHandoff = serde_json::from_str(&raw).unwrap();
                assert!(h.processed, "all handoffs should be marked processed");
            }
        }
    }

    #[test]
    fn batch_fast_marks_empty_handoff() {
        use std::fs;

        let dir = TempDir::new().expect("create temp dir");

        // Empty handoff: 0 decisions, 0 action items.
        let handoff = MeetingHandoff {
            schema_version: 2,
            topic: "Empty chat".to_string(),
            started_at: "2026-05-01T00:00:00Z".to_string(),
            closed_at: "2026-05-01T00:01:00Z".to_string(),
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
            next_owner: None,
            artifacts: Vec::new(),
            goal: None,
            next_actor: None,
            applied_templates: Vec::new(),
            history_truncated_count: 0,
            partial_reason: None,
            risks: vec![],
            disagreements: vec![],
        };
        let path = dir.path().join("handoff-2026-05-01T00-01-00_00-00.json");
        fs::write(&path, serde_json::to_string_pretty(&handoff).unwrap()).unwrap();

        let mut board = GoalBoard::new();
        let count = check_meeting_handoffs(&mut board, dir.path(), dir.path())
            .expect("fast-mark should succeed");

        // Empty handoff is fast-marked → 0 items created, but file is processed.
        assert_eq!(count, 0, "empty handoff should produce 0 created items");
        assert!(board.active.is_empty(), "no goals should be added");
        assert!(board.backlog.is_empty(), "no backlog items should be added");

        let reloaded: MeetingHandoff =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            reloaded.processed,
            "empty handoff must be fast-marked as processed"
        );
    }

    #[test]
    fn batch_cap_at_10_handoffs() {
        use std::fs;

        let dir = TempDir::new().expect("create temp dir");

        // Create 12 handoffs, each with a unique decision.
        for i in 0..12u8 {
            let handoff = sample_handoff(vec![sample_decision(&format!("Decision {i}"))]);
            let ts = format!("handoff-2026-06-{:02}T00-00-00_00-00.json", i + 1);
            fs::write(
                dir.path().join(&ts),
                serde_json::to_string_pretty(&handoff).unwrap(),
            )
            .unwrap();
        }

        let mut board = GoalBoard::new();
        let count = check_meeting_handoffs(&mut board, dir.path(), dir.path())
            .expect("capped batch should succeed");

        // Only first 10 should be processed (batch cap).
        assert_eq!(count, 10, "batch cap should limit to 10 handoffs per cycle");

        // Exactly 10 files should be marked processed, 2 should remain.
        let mut processed_count = 0u32;
        let mut unprocessed_count = 0u32;
        for entry in fs::read_dir(dir.path()).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name().to_string_lossy().starts_with("handoff-") {
                let raw = fs::read_to_string(entry.path()).unwrap();
                let h: MeetingHandoff = serde_json::from_str(&raw).unwrap();
                if h.processed {
                    processed_count += 1;
                } else {
                    unprocessed_count += 1;
                }
            }
        }
        assert_eq!(
            processed_count, 10,
            "10 handoffs should be marked processed"
        );
        assert_eq!(unprocessed_count, 2, "2 handoffs should remain unprocessed");
    }

    // -----------------------------------------------------------------
    // reap_old_handoffs tests for #2268.
    // -----------------------------------------------------------------

    #[test]
    fn reap_old_handoffs_deletes_old_processed() {
        use std::fs;
        use std::time::{Duration, SystemTime};

        let dir = TempDir::new().expect("create temp dir");

        // Create a processed handoff file.
        let mut handoff = sample_handoff(vec![sample_decision("Old processed")]);
        handoff.processed = true;
        let path = dir.path().join("handoff-2026-01-01T00-00-00_00-00.json");
        fs::write(&path, serde_json::to_string_pretty(&handoff).unwrap()).unwrap();

        // Set mtime to 8 days ago (> 7 day threshold).
        let old_time = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
        let times = fs::FileTimes::new().set_modified(old_time);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(times)
            .unwrap();

        let deleted = reap_old_handoffs(dir.path()).expect("reap should succeed");
        assert_eq!(deleted, 1, "should delete one old processed handoff");
        assert!(!path.exists(), "old processed file should be deleted");
    }

    #[test]
    fn reap_old_handoffs_preserves_recent_and_unprocessed() {
        use std::fs;
        use std::time::{Duration, SystemTime};

        let dir = TempDir::new().expect("create temp dir");

        let old_time = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);

        // (a) Processed + recent — should be preserved.
        let mut ha = sample_handoff(vec![sample_decision("Recent processed")]);
        ha.processed = true;
        let path_a = dir.path().join("handoff-2026-06-10T00-00-00_00-00.json");
        fs::write(&path_a, serde_json::to_string_pretty(&ha).unwrap()).unwrap();
        // mtime is now (recent) — don't touch it.

        // (b) Unprocessed + old — should be preserved (not yet consumed).
        let hb = sample_handoff(vec![sample_decision("Old unprocessed")]);
        let path_b = dir.path().join("handoff-2026-01-02T00-00-00_00-00.json");
        fs::write(&path_b, serde_json::to_string_pretty(&hb).unwrap()).unwrap();
        let times_old = fs::FileTimes::new().set_modified(old_time);
        fs::File::options()
            .write(true)
            .open(&path_b)
            .unwrap()
            .set_times(times_old)
            .unwrap();

        // (c) Processed + old — should be deleted.
        let mut hc = sample_handoff(vec![sample_decision("Old processed deletable")]);
        hc.processed = true;
        let path_c = dir.path().join("handoff-2026-01-03T00-00-00_00-00.json");
        fs::write(&path_c, serde_json::to_string_pretty(&hc).unwrap()).unwrap();
        let times_old2 = fs::FileTimes::new().set_modified(old_time);
        fs::File::options()
            .write(true)
            .open(&path_c)
            .unwrap()
            .set_times(times_old2)
            .unwrap();

        let deleted = reap_old_handoffs(dir.path()).expect("reap should succeed");
        assert_eq!(deleted, 1, "only old+processed should be deleted");
        assert!(path_a.exists(), "recent processed should be preserved");
        assert!(path_b.exists(), "old unprocessed should be preserved");
        assert!(!path_c.exists(), "old processed should be deleted");
    }

    #[test]
    fn reap_old_handoffs_empty_dir() {
        let dir = TempDir::new().expect("create temp dir");
        let deleted = reap_old_handoffs(dir.path()).expect("reap on empty dir should succeed");
        assert_eq!(deleted, 0, "nothing to reap in empty dir");
    }
}
