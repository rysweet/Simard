//! Hermetic tests for the authoritative goal-board store (issue #1).
//!
//! Every test uses an isolated `TempDir` as `SIMARD_STATE_ROOT`, so the
//! cross-process `flock`, the atomic file write, and the tombstone file are all
//! exercised against a private state root — no shared global state, no touching
//! the real `~/.simard`.

use std::collections::HashSet;

use tempfile::TempDir;

use super::*;
use crate::goal_curation::completion_gate::EvidenceSource;
use crate::goal_curation::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, WipRef};
use crate::ooda_loop::{load_tombstones, tombstone_goals};

/// Build an active goal in the `NotStarted` state, optionally cross-repo.
fn goal(id: &str, priority: u32, repo: Option<&str>) -> ActiveGoal {
    ActiveGoal::new(id, format!("do {id}"), priority).with_repo(repo.map(str::to_string))
}

/// A board with the given active goals and no backlog.
fn board(goals: Vec<ActiveGoal>) -> GoalBoard {
    GoalBoard {
        active: goals,
        backlog: Vec::new(),
    }
}

/// Evidence source that certifies a goal complete iff it carries a `pr` wip_ref
/// (models "the referenced PR is merged and any linked issue is closed"). Used
/// to exercise the every-cycle done-gate sweep hermetically.
struct FakePrMergedEvidence;

impl EvidenceSource for FakePrMergedEvidence {
    fn any_pr_merged(&self, goal: &ActiveGoal) -> crate::error::SimardResult<bool> {
        Ok(goal
            .wip_refs
            .iter()
            .any(|r| r.kind.eq_ignore_ascii_case("pr")))
    }
    fn issue_closed(&self, _goal: &ActiveGoal) -> crate::error::SimardResult<bool> {
        Ok(true)
    }
    fn is_deployed(&self, _goal: &ActiveGoal) -> crate::error::SimardResult<bool> {
        Ok(true)
    }
}

fn pr_ref(num: &str) -> WipRef {
    WipRef {
        kind: "pr".to_string(),
        ref_id: num.to_string(),
        label: format!("PR #{num}"),
        url: None,
    }
}

#[test]
fn load_is_empty_when_no_file() {
    let tmp = TempDir::new().unwrap();
    let state = load(tmp.path());
    assert!(state.board.active.is_empty());
    assert!(state.board.backlog.is_empty());
}

#[test]
fn store_round_trips_read_your_writes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Commit a board via the RMW primitive.
    mutate(root, |s| {
        s.board = board(vec![
            goal("alpha", 1, None),
            goal("beta", 2, Some("ladybug-rust")),
        ]);
    })
    .unwrap();

    // load() returns exactly the last committed board.
    let loaded = load(root);
    assert_eq!(loaded.board.active.len(), 2);
    assert_eq!(loaded.board.active[0].id, "alpha");
    assert_eq!(loaded.version, STORE_VERSION);

    // A second write is immediately visible (read-your-writes).
    mutate(root, |s| {
        s.board.active.retain(|g| g.id != "alpha");
    })
    .unwrap();
    let loaded = load(root);
    assert_eq!(loaded.board.active.len(), 1);
    assert_eq!(loaded.board.active[0].id, "beta");
}

#[test]
fn reconcile_preserves_operator_added_goal() {
    // Persisted (authoritative) file has an operator-added goal the daemon has
    // never seen; the daemon's in-flight board carries only its own goal.
    let persisted = board(vec![goal("operator-goal", 1, None)]);
    let in_flight = board(vec![goal("daemon-goal", 2, None)]);
    let tombstones = HashSet::new();

    let merged = reconcile(&persisted, &in_flight, &tombstones);
    let ids: HashSet<&str> = merged.active.iter().map(|g| g.id.as_str()).collect();
    assert!(
        ids.contains("operator-goal"),
        "operator add must survive curation merge"
    );
    assert!(ids.contains("daemon-goal"));
}

#[test]
fn reconcile_drops_tombstoned_goal_even_if_daemon_still_has_it() {
    // Operator removed the goal from the authoritative file (so `persisted`
    // lacks it) AND tombstoned it — but the daemon's in-flight board still
    // carries it (it loaded before the remove). The reconcile MUST drop it.
    let persisted = board(vec![goal("keep", 1, None)]);
    let in_flight = board(vec![goal("keep", 1, None), goal("removed", 2, None)]);
    let mut tombstones = HashSet::new();
    tombstones.insert("removed".to_string());

    let merged = reconcile(&persisted, &in_flight, &tombstones);
    let ids: HashSet<&str> = merged.active.iter().map(|g| g.id.as_str()).collect();
    assert!(ids.contains("keep"));
    assert!(
        !ids.contains("removed"),
        "a tombstoned goal must never be clobbered back onto the board"
    );
}

#[test]
fn filter_tombstoned_drops_active_and_backlog() {
    let mut b = board(vec![goal("a", 1, None), goal("b", 2, None)]);
    b.backlog.push(BacklogItem {
        id: "c".to_string(),
        description: "c".to_string(),
        source: "test".to_string(),
        score: 0.5,
    });
    let mut ts = HashSet::new();
    ts.insert("b".to_string());
    ts.insert("c".to_string());

    let filtered = filter_tombstoned(b, &ts);
    assert_eq!(filtered.active.len(), 1);
    assert_eq!(filtered.active[0].id, "a");
    assert!(filtered.backlog.is_empty());
}

#[test]
fn commit_cycle_tombstones_and_persists() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Seed the authoritative file with two goals.
    mutate(root, |s| {
        s.board = board(vec![goal("a", 1, None), goal("b", 2, None)]);
    })
    .unwrap();

    // Daemon commits its post-cycle board, tombstoning goal "b" (archived).
    let in_flight = board(vec![goal("a", 1, None), goal("b", 2, None)]);
    let tracker = NoProgressTracker::new();
    let reconciled = commit_cycle(root, &in_flight, &tracker, &["b".to_string()]).unwrap();

    let ids: HashSet<&str> = reconciled.active.iter().map(|g| g.id.as_str()).collect();
    assert!(ids.contains("a"));
    assert!(!ids.contains("b"), "committed goal must be tombstoned out");

    // The tombstone is durable, and a subsequent reload never resurrects "b"
    // even if a stale in-flight board still has it.
    assert!(load_tombstones(root).contains("b"));
    let reloaded = load(root);
    let ids: HashSet<&str> = reloaded
        .board
        .active
        .iter()
        .map(|g| g.id.as_str())
        .collect();
    assert!(!ids.contains("b"));
}

#[test]
fn tombstone_prevents_reseed_from_memory_recall() {
    // Simulate a "recalled" board that tries to reintroduce a completed goal.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Operator completes/removes "supply-chain-hardening" — tombstone it.
    tombstone_goals(root, &["supply-chain-hardening".to_string()]).unwrap();

    // A recall path proposes a board that still contains it.
    let recalled = board(vec![
        goal("supply-chain-hardening", 1, Some("ladybug-rust")),
        goal("fresh-goal", 2, None),
    ]);
    let tombstones = load_tombstones(root);
    let filtered = filter_tombstoned(recalled, &tombstones);

    let ids: HashSet<&str> = filtered.active.iter().map(|g| g.id.as_str()).collect();
    assert!(
        !ids.contains("supply-chain-hardening"),
        "recall must not resurrect a tombstoned goal"
    );
    assert!(ids.contains("fresh-goal"));
}

#[test]
fn sweep_completes_cross_repo_goal_with_evidence() {
    // Two not-started goals; only the cross-repo one carries a merged-PR ref.
    let mut b = board(vec![
        {
            let mut g = goal("ladybug-hardening", 1, Some("ladybug-rust"));
            g.wip_refs.push(pr_ref("1"));
            g
        },
        goal("no-evidence-yet", 2, None),
    ]);

    let completed = sweep_done_goals(&mut b, &FakePrMergedEvidence);
    assert_eq!(completed, vec!["ladybug-hardening".to_string()]);

    // The cross-repo goal is now Completed; the other is untouched.
    let done = b
        .active
        .iter()
        .find(|g| g.id == "ladybug-hardening")
        .unwrap();
    assert_eq!(done.status, GoalProgress::Completed);
    let other = b.active.iter().find(|g| g.id == "no-evidence-yet").unwrap();
    assert_eq!(other.status, GoalProgress::NotStarted);
}

#[test]
fn four_ladybug_goals_auto_complete_via_done_gate_and_never_return() {
    // Headline production scenario (issue #1): the four ladybug supply-chain
    // goals — each objectively DONE on a *different governed repo* — are
    // auto-completed by the every-cycle CROSS-REPO done-gate, tombstoned as they
    // leave the board, and can NEVER be resurrected by a later memory-recall /
    // curation pass or a daemon restart. A Simard-repo control goal with no
    // evidence stays active throughout, proving the gate is evidence-driven and
    // not a blanket sweep.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    let ladybug_ids = [
        "ladybug-rust-harden",
        "ladybug-graph-harden",
        "lbug-patched-scope",
        "ladybug-rust-audit",
    ];

    // Seed four cross-repo ladybug goals (each carrying merged-PR evidence) plus
    // one Simard-repo control with none, directly into the authoritative store.
    mutate(root, |s| {
        s.board = board(vec![
            {
                let mut g = goal("ladybug-rust-harden", 1, Some("ladybug-rust"));
                g.wip_refs.push(pr_ref("1"));
                g
            },
            {
                let mut g = goal("ladybug-graph-harden", 2, Some("ladybug-graph-rs"));
                g.wip_refs.push(pr_ref("1"));
                g
            },
            {
                let mut g = goal("lbug-patched-scope", 3, Some("lbug-patched"));
                g.wip_refs.push(pr_ref("1"));
                g
            },
            {
                let mut g = goal("ladybug-rust-audit", 4, Some("ladybug-rust"));
                g.wip_refs.push(pr_ref("2"));
                g
            },
            goal("simard-control", 5, None),
        ]);
    })
    .unwrap();

    // ── Daemon cycle: reload the authoritative board, run the every-cycle
    // done-gate over the WHOLE active set (cross-repo aware). ──
    let mut in_flight = load(root).board;
    let completed = sweep_done_goals(&mut in_flight, &FakePrMergedEvidence);
    let done: HashSet<&str> = completed.iter().map(String::as_str).collect();
    for id in ladybug_ids {
        assert!(
            done.contains(id),
            "the cross-repo done-gate must auto-complete ladybug goal {id}"
        );
    }
    assert!(
        !done.contains("simard-control"),
        "a goal without completion evidence must stay active"
    );

    // The daemon drops the just-completed goals and commits the cycle,
    // tombstoning every id that left the board.
    in_flight.active.retain(|g| !done.contains(g.id.as_str()));
    let reconciled = commit_cycle(root, &in_flight, &NoProgressTracker::new(), &completed).unwrap();
    let live: HashSet<&str> = reconciled.active.iter().map(|g| g.id.as_str()).collect();
    for id in ladybug_ids {
        assert!(
            !live.contains(id),
            "a done ladybug goal ({id}) must LEAVE the board once completed"
        );
    }
    assert!(
        live.contains("simard-control"),
        "the active control goal must survive the cycle"
    );

    // All four are durably tombstoned.
    let tombstones = load_tombstones(root);
    for id in ladybug_ids {
        assert!(tombstones.contains(id), "{id} must be durably tombstoned");
    }

    // A later memory-recall / curation pass proposing all four again cannot
    // resurrect them — neither the load-time tombstone filter nor the cycle
    // reconcile lets a tombstoned goal back onto the board.
    let recalled = board(
        ladybug_ids
            .iter()
            .enumerate()
            .map(|(i, id)| goal(id, i as u32 + 1, Some("ladybug-rust")))
            .collect(),
    );
    let filtered = filter_tombstoned(recalled.clone(), &tombstones);
    assert!(
        filtered.active.is_empty(),
        "recall must not re-seed any completed ladybug goal"
    );
    let reconciled2 = reconcile(&reconciled, &recalled, &tombstones);
    let live2: HashSet<&str> = reconciled2.active.iter().map(|g| g.id.as_str()).collect();
    for id in ladybug_ids {
        assert!(
            !live2.contains(id),
            "curation reconcile must not resurrect tombstoned ladybug goal {id}"
        );
    }

    // And a fresh restart (a plain reload of goal_board.json) shows them still
    // gone — the "re-litigated every cycle" loop is closed for good.
    let after_restart = load(root).board;
    for id in ladybug_ids {
        assert!(
            !after_restart.active.iter().any(|g| g.id == id),
            "{id} must stay off the board across a daemon restart"
        );
    }
    assert!(
        after_restart
            .active
            .iter()
            .any(|g| g.id == "simard-control"),
        "the control goal must persist across the restart"
    );
}

#[test]
fn no_progress_tracker_persists_across_simulated_restart() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Cycle 1 (process instance A): one no-action cycle on "stuck".
    mutate(root, |s| {
        s.no_progress.record_no_action("stuck");
    })
    .unwrap();

    // Cycle 2 (still A): a second no-action cycle.
    mutate(root, |s| {
        s.no_progress.record_no_action("stuck");
    })
    .unwrap();

    // --- Daemon process restart: a fresh load must recover the counter (2),
    // NOT reset it to 0. This is the bug the production report describes. ---
    let recovered = load(root);
    assert_eq!(
        recovered.no_progress.consecutive("stuck"),
        2,
        "the no-progress counter must survive a daemon restart"
    );

    // Cycle 3 (process instance B): the third no-action cycle crosses the
    // breaker threshold *because the counter carried over the restart*.
    let count = mutate(root, |s| s.no_progress.record_no_action("stuck")).unwrap();
    assert_eq!(
        count, 3,
        "the counter accumulates across the restart to reach the threshold"
    );
}

#[test]
fn no_progress_breaker_fires_after_restart_using_persisted_counter() {
    use crate::goal_curation::no_progress_breaker::{NoProgressResolution, StuckGoalDisposition};

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Two no-action cycles before the restart (below the threshold of 3).
    mutate(root, |s| {
        s.no_progress.record_no_action("stuck");
    })
    .unwrap();
    mutate(root, |s| {
        s.no_progress.record_no_action("stuck");
    })
    .unwrap();

    // --- Restart: reload the persisted tracker from the authoritative store. ---
    let mut recovered = load(root).no_progress;
    assert_eq!(recovered.consecutive("stuck"), 2);

    // The third no-action cycle *after the restart* crosses the threshold and
    // the breaker FIRES with a terminal resolution (here: evidence present →
    // MarkDone). Before this fix the counter reset to 0 each restart, so the
    // breaker could never reach the threshold — exactly the "0 log lines in
    // 4.5h" production symptom.
    let resolution = recovered.record_and_resolve("stuck", 3, || StuckGoalDisposition::Done);
    assert_eq!(resolution, NoProgressResolution::MarkDone);
    assert!(
        resolution.is_terminal(),
        "the no-progress breaker must fire once the counter — persisted across the restart — reaches the threshold"
    );

    // Persist the cleared counter and confirm the terminal firing reset it.
    mutate(root, |s| {
        s.no_progress = recovered.clone();
    })
    .unwrap();
    assert_eq!(load(root).no_progress.consecutive("stuck"), 0);
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn load_or_migrate_writes_file_from_memory_snapshot() {
    use crate::memory_ipc::launch_writer_bridge;
    use crate::test_support::hermetic::HermeticState;

    let state = HermeticState::new();
    let root = state.state_root();

    // Seed the legacy cognitive-memory snapshot (the pre-#1 source of truth).
    let bridge = launch_writer_bridge(root).expect("writer bridge");
    crate::goal_curation::save_goal_board(&board(vec![goal("legacy", 1, None)]), bridge.ops())
        .expect("seed memory snapshot");

    // First adoption: no goal_board.json yet, so migrate from memory.
    assert!(!store_path(root).exists());
    let migrated = load_or_migrate(root, bridge.ops()).expect("migrate");
    assert!(
        store_path(root).exists(),
        "migration must create the authoritative file"
    );
    let ids: HashSet<&str> = migrated
        .board
        .active
        .iter()
        .map(|g| g.id.as_str())
        .collect();
    assert!(
        ids.contains("legacy"),
        "no live goal may be lost on migration"
    );

    // Second call is a pure read of the now-authoritative file.
    let again = load_or_migrate(root, bridge.ops()).expect("reload");
    assert_eq!(again.board.active.len(), 1);
}
