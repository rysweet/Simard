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
    let reconciled = commit_cycle(root, &in_flight, &tracker, 1, &["b".to_string()]).unwrap();

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
    let reconciled =
        commit_cycle(root, &in_flight, &NoProgressTracker::new(), 1, &completed).unwrap();
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
    use crate::memory_ipc::launch_writer_client;
    use crate::test_support::hermetic::HermeticState;

    let state = HermeticState::new();
    let root = state.state_root();

    // Seed the legacy cognitive-memory snapshot (the pre-#1 source of truth).
    let memory = launch_writer_client(root).expect("writer memory");
    crate::goal_curation::save_goal_board(&board(vec![goal("legacy", 1, None)]), memory.ops())
        .expect("seed memory snapshot");

    // First adoption: no goal_board.json yet, so migrate from memory.
    assert!(!store_path(root).exists());
    let migrated = load_or_migrate(root, memory.ops()).expect("migrate");
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
    let again = load_or_migrate(root, memory.ops()).expect("reload");
    assert_eq!(again.board.active.len(), 1);
}

// --- perpetual/standing-goal self-heal on load (issue #2589) ----------------

/// A standing/perpetual goal (issues #2580/#2589): recognised by the *same*
/// `is_perpetual()` flag the non-completability path keys on.
fn standing_goal(id: &str) -> ActiveGoal {
    let g = ActiveGoal::new(
        id,
        format!("STANDING PERPETUAL goal — {id}; never mark complete"),
        5,
    );
    assert!(
        g.is_perpetual(),
        "fixture must be recognised as standing/perpetual by the shared #2580/#2589 flag"
    );
    g
}

#[test]
fn heals_stale_perpetual_no_progress_block() {
    use crate::goal_curation::no_progress_breaker::no_progress_blocked_reason;

    // A prior daemon build parked a standing/perpetual goal with the
    // [OODA-SAFEGUARD] no-progress sentinel. On load it must SELF-HEAL back to an
    // actionable, re-dispatchable state (NotStarted) — a standing goal must never
    // stay parked or require a manual unblock.
    let mut g = standing_goal("continuously-research-and-improve");
    g.status = GoalProgress::Blocked(no_progress_blocked_reason(3));

    let healed = heal_stale_no_progress_blocks(board(vec![g]));

    assert_eq!(
        healed.active[0].status,
        GoalProgress::NotStarted,
        "a stale no-progress block on a perpetual goal must be cleared to NotStarted"
    );
}

#[test]
fn leaves_non_perpetual_no_progress_block_intact() {
    use crate::goal_curation::no_progress_breaker::no_progress_blocked_reason;

    // Regression: a NORMAL goal parked by the same safeguard is a legitimate
    // human-review block and must stay Blocked — self-heal never touches it.
    let mut g = goal("normal-stuck", 1, None);
    assert!(!g.is_perpetual());
    let reason = no_progress_blocked_reason(3);
    g.status = GoalProgress::Blocked(reason.clone());

    let healed = heal_stale_no_progress_blocks(board(vec![g]));

    assert_eq!(
        healed.active[0].status,
        GoalProgress::Blocked(reason),
        "a normal goal's no-progress block must be preserved (no regression)"
    );
}

#[test]
fn leaves_perpetual_non_safeguard_block_intact() {
    use crate::goal_curation::no_progress_breaker::is_no_progress_marker;

    // A perpetual goal Blocked for a DIFFERENT reason (operator hold / scope /
    // dependency) is a legitimate block a human set. Self-heal keys STRICTLY on
    // the no-progress sentinel, so this block must survive untouched.
    let mut g = standing_goal("standing-on-hold");
    let reason = "operator hold: paused pending design review".to_string();
    assert!(
        !is_no_progress_marker(&reason),
        "control reason must NOT be a no-progress sentinel"
    );
    g.status = GoalProgress::Blocked(reason.clone());

    let healed = heal_stale_no_progress_blocks(board(vec![g]));

    assert_eq!(
        healed.active[0].status,
        GoalProgress::Blocked(reason),
        "a non-safeguard block on a perpetual goal must be preserved"
    );
}

#[test]
fn heal_stale_no_progress_blocks_is_idempotent() {
    use crate::goal_curation::no_progress_breaker::no_progress_blocked_reason;

    let mut g = standing_goal("continuously-research");
    g.status = GoalProgress::Blocked(no_progress_blocked_reason(4));

    let once = heal_stale_no_progress_blocks(board(vec![g]));
    assert_eq!(once.active[0].status, GoalProgress::NotStarted);

    let twice = heal_stale_no_progress_blocks(once.clone());
    assert_eq!(
        twice.active[0].status,
        GoalProgress::NotStarted,
        "a second heal pass must be a no-op"
    );
}

// ---------------------------------------------------------------------------
// Durable, brain-relative OODA cycle counter (issue #1)
//
// The OODA "cycle #" must reflect the BRAIN's total lived cognition — a
// monotonic counter persisted in the durable goal-board store — NOT the
// current daemon process's uptime. Before this feature the counter lived only
// in `OodaState::cycle_count`, so every daemon restart (a frequent deploy)
// reset it to 1 and the dashboard perpetually showed "Cycle #1".
//
// CONTRACT specified by these tests:
//   * `PersistentGoalState` carries `#[serde(default)] pub cycle_count: u32`.
//   * `commit_cycle(state_root, in_flight, tracker, cycle_count, new_tombstones)`
//     persists the cycle number inside the same flock'd atomic read-modify-write,
//     applying a MONOTONIC guard (`s.cycle_count = s.cycle_count.max(cycle_count)`)
//     so the durable counter can never rewind.
//   * `load()` returns the last committed `cycle_count` (read-your-writes),
//     surviving a simulated restart.
//   * A fresh brain (no file) loads `cycle_count == 0`; the daemon's startup
//     seed + first `+= 1` makes the first cycle #1.
//   * A legacy file lacking the field deserialises to `0` (serde default) and
//     is re-stamped on the next commit (self-healing, no migration code).
// ---------------------------------------------------------------------------

#[test]
fn cycle_count_round_trips_through_store() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    mutate(root, |s| {
        s.cycle_count = 7;
    })
    .unwrap();

    let loaded = load(root);
    assert_eq!(
        loaded.cycle_count, 7,
        "the durable cycle counter must round-trip through the store"
    );
    assert_eq!(loaded.version, STORE_VERSION);
}

#[test]
fn fresh_brain_starts_at_zero_then_first_cycle_is_one() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // A brand-new brain has no persisted state: no lived cycles yet.
    let fresh = load(root);
    assert_eq!(
        fresh.cycle_count, 0,
        "a fresh brain has zero prior cycles (the daemon seeds OodaState from this)"
    );

    // The daemon's first cycle: seed (0) + 1 == 1 (mirrors `state.cycle_count += 1`).
    let b = board(vec![goal("alpha", 1, None)]);
    let tracker = NoProgressTracker::new();
    let mut cycle = fresh.cycle_count;
    cycle += 1;
    commit_cycle(root, &b, &tracker, cycle, &[]).unwrap();
    assert_eq!(
        load(root).cycle_count,
        1,
        "a fresh brain's first cycle must be #1"
    );

    // The second cycle advances to #2.
    cycle += 1;
    commit_cycle(root, &b, &tracker, cycle, &[]).unwrap();
    assert_eq!(
        load(root).cycle_count,
        2,
        "the counter increments on each cycle"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// #4419 — course-correct a blocked goal by REWRITING its unmeasurable done-gate
// into a machine-checkable first slice, atomically under the store `flock`.
//
// A goal like "raise Simard test coverage to 70%" churns because "70% coverage"
// is not a finish line the completion gate can read: with no tracked PR/issue it
// can never certify done, so after repeated no-progress cycles it is demoted to a
// Blocked cooldown with no engineer assigned. The self-correction: rewrite the
// done-criteria to ONE concrete under-tested module with a bounded threshold,
// attach an observable tracking ref the gate CAN read, assign an owner, and
// transition Blocked(..) -> NotStarted so the goal re-enters the active list.
//
// RED: every `super::` symbol below (`FirstSliceTarget`,
// `rewrite_blocked_goal_done_gate`, `CorrectionOutcome`, `CorrectionRejected`,
// `validate_threshold` / `validate_module_path` / `validate_owner`) is the
// executable contract the #4419 implementation must satisfy — none exist yet, so
// the crate test build fails to compile until the feature lands. That compile
// failure IS the red state of red→green→refactor.
// ════════════════════════════════════════════════════════════════════════════

/// The real blocked goal from the triage task: raise Simard test coverage to 70%.
const COVERAGE_GOAL_ID: &str = "audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a";

/// An observable tracking issue whose CLOSED state the completion gate can read —
/// this is what makes the rewritten done-gate machine-checkable.
fn issue_ref(num: &str) -> WipRef {
    WipRef {
        kind: "issue".to_string(),
        ref_id: num.to_string(),
        label: format!("issue #{num}"),
        url: Some(format!("https://github.com/rysweet/Simard/issues/{num}")),
    }
}

/// Seed one blocked coverage goal — demoted after 5 consecutive no-progress
/// cycles, with no engineer assigned — into the isolated store at `root`.
fn seed_blocked_coverage_goal(root: &std::path::Path) {
    use crate::goal_curation::no_progress_breaker::no_progress_blocked_reason;
    let mut g = ActiveGoal::new(
        COVERAGE_GOAL_ID,
        "Audit Simard's test coverage and raise it to 70%",
        1,
    );
    g.status = GoalProgress::Blocked(no_progress_blocked_reason(5));
    g.assigned_to = None; // no engineer assigned — part of the diagnostic WHY
    mutate(root, |s| s.board = board(vec![g])).unwrap();
}

/// A well-formed first-slice target: one named under-tested module, a bounded
/// coverage threshold, an owner, and an observable tracking issue.
fn valid_target() -> FirstSliceTarget {
    FirstSliceTarget::new(
        "src/signal_conversation/channel.rs",
        70,
        "alice",
        issue_ref("4420"),
    )
    .expect("a well-formed first-slice target validates")
}

#[test]
fn rewrite_transitions_blocked_goal_to_a_machine_checkable_first_slice() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    seed_blocked_coverage_goal(root);

    let outcome = rewrite_blocked_goal_done_gate(root, COVERAGE_GOAL_ID, &valid_target())
        .expect("the store mutate succeeds");
    let corrected = match outcome {
        CorrectionOutcome::Corrected(g) => g,
        CorrectionOutcome::Rejected(r) => panic!("expected a correction, got {r:?}"),
    };

    // The correction leaves the Blocked cooldown and re-enters the active list.
    assert_eq!(
        corrected.status,
        GoalProgress::NotStarted,
        "a corrected goal transitions Blocked(..) -> NotStarted so it is re-dispatchable"
    );
    assert_eq!(
        corrected.assigned_to.as_deref(),
        Some("alice"),
        "an owner is assigned so someone is responsible for moving it"
    );

    // The finish line is rewritten to a concrete, bounded, per-module slice ...
    assert!(
        corrected
            .description
            .contains("src/signal_conversation/channel.rs"),
        "the done-criteria names one concrete under-tested module: {:?}",
        corrected.description
    );
    assert!(
        corrected.description.contains("70"),
        "the done-criteria carries the bounded coverage threshold: {:?}",
        corrected.description
    );

    // ... and it becomes machine-checkable by attaching an observable tracking ref.
    assert!(
        corrected
            .wip_refs
            .iter()
            .any(|r| r.kind.eq_ignore_ascii_case("issue") && r.ref_id == "4420"),
        "an observable tracking ref is attached so the completion gate can certify done"
    );

    // The whole read-modify-write persisted atomically inside one `mutate` window
    // (no TOCTOU): the reloaded goal equals the returned correction verbatim.
    let reloaded = load(root);
    assert_eq!(reloaded.board.active.len(), 1);
    assert_eq!(
        reloaded.board.active[0], corrected,
        "the corrected goal is persisted verbatim (single flock window, read-your-writes)"
    );
}

#[test]
fn rewrite_rejects_an_unknown_goal_without_touching_the_board() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    seed_blocked_coverage_goal(root);
    let before = load(root).board.active;

    let outcome = rewrite_blocked_goal_done_gate(root, "no-such-goal", &valid_target())
        .expect("the store mutate succeeds");
    assert_eq!(
        outcome,
        CorrectionOutcome::Rejected(CorrectionRejected::GoalNotFound {
            goal_id: "no-such-goal".to_string(),
        }),
        "an unknown goal id is rejected, never fabricated"
    );
    assert_eq!(
        load(root).board.active,
        before,
        "a rejected correction leaves every active goal untouched"
    );
}

#[test]
fn rewrite_rejects_a_goal_that_is_not_blocked() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    // The same goal, but active (NotStarted) — there is no block to course-correct.
    let g = ActiveGoal::new(
        COVERAGE_GOAL_ID,
        "Audit Simard's test coverage and raise it to 70%",
        1,
    );
    mutate(root, |s| s.board = board(vec![g])).unwrap();

    let outcome = rewrite_blocked_goal_done_gate(root, COVERAGE_GOAL_ID, &valid_target())
        .expect("the store mutate succeeds");
    match outcome {
        CorrectionOutcome::Rejected(CorrectionRejected::NotBlocked { goal_id, .. }) => {
            assert_eq!(goal_id, COVERAGE_GOAL_ID);
        }
        other => panic!("a non-blocked goal must be rejected as NotBlocked, got {other:?}"),
    }
    assert_eq!(
        load(root).board.active[0].status,
        GoalProgress::NotStarted,
        "a rejected correction must not mutate the goal"
    );
}

#[test]
fn threshold_must_be_a_percentage_between_0_and_100() {
    assert!(validate_threshold(0).is_ok());
    assert!(validate_threshold(70).is_ok());
    assert!(validate_threshold(100).is_ok());
    assert_eq!(
        validate_threshold(101),
        Err(CorrectionRejected::ThresholdOutOfRange { got: 101 }),
        "a threshold above 100% is not a reachable coverage target"
    );
    // A bad threshold makes the whole target construction fail — it is never
    // persisted; a malformed input routes the triage brain to the ask-operator path.
    assert!(matches!(
        FirstSliceTarget::new("src/lib.rs", 250, "alice", issue_ref("1")),
        Err(CorrectionRejected::ThresholdOutOfRange { got: 250 })
    ));
}

#[test]
fn module_path_rejects_traversal_absolute_and_shell_metacharacters() {
    assert!(validate_module_path("src/goal_curation/completion_gate.rs").is_ok());

    for bad in [
        "",                     // empty
        "../etc/passwd",        // parent-dir traversal
        "src/../../secrets",    // traversal mid-path
        "/etc/passwd",          // absolute path
        "src/mod.rs; rm -rf /", // shell command separator
        "src/$(whoami).rs",     // command substitution
        "src/a|b.rs",           // pipe
        "src/a\nb.rs",          // embedded newline
        // Marker-smuggling: `docs-only` / `documentation-only` survive the
        // char-set (letters + `-`) but would splice into the persisted
        // description and flip the goal to non-self-affecting, skipping the
        // deploy-aware done-gate. Must be rejected fail-closed.
        "docs-only",
        "src/docs-only/mod.rs",
        "documentation-only",
    ] {
        assert!(
            matches!(
                validate_module_path(bad),
                Err(CorrectionRejected::UnsafeModulePath { .. })
            ),
            "module path {bad:?} must be rejected as unsafe"
        );
    }
}

#[test]
fn owner_rejects_empty_and_log_injection_payloads() {
    assert!(validate_owner("alice").is_ok());
    for bad in ["", "alice\nBcc: evil", "a\tb", "x\r\ny"] {
        assert!(
            matches!(
                validate_owner(bad),
                Err(CorrectionRejected::InvalidOwner { .. })
            ),
            "owner {bad:?} must be rejected (control chars / newlines / empty)"
        );
    }
}

#[test]
fn tracking_ref_rejects_empty_control_chars_and_smuggled_standing_markers() {
    // A well-formed tracking ref (all fields present, single-line, no marker)
    // validates and reaches the board.
    assert!(validate_tracking_ref(&issue_ref("4420")).is_ok());

    // Each field is guarded: an empty or control-char/newline-bearing kind,
    // ref_id, or label is rejected before it can be persisted or logged.
    let bad_field = |field: &str, value: &str| WipRef {
        kind: if field == "kind" {
            value.to_string()
        } else {
            "issue".to_string()
        },
        ref_id: if field == "ref_id" {
            value.to_string()
        } else {
            "4420".to_string()
        },
        label: if field == "label" {
            value.to_string()
        } else {
            "issue #4420".to_string()
        },
        url: None,
    };
    for field in ["kind", "ref_id", "label"] {
        for bad in ["", "   ", "a\nb", "a\tb", "x\r\ny"] {
            assert!(
                matches!(
                    validate_tracking_ref(&bad_field(field, bad)),
                    Err(CorrectionRejected::InvalidTrackingRef { field: f, .. }) if f == field
                ),
                "tracking-ref {field} {bad:?} must be rejected (empty / control chars / newlines)"
            );
        }
    }

    // An over-length field is rejected even when it is otherwise control-free.
    let long = "x".repeat(MAX_TRACKING_REF_FIELD_LEN + 1);
    assert!(matches!(
        validate_tracking_ref(&bad_field("label", &long)),
        Err(CorrectionRejected::InvalidTrackingRef { field: "label", .. })
    ));

    // The security-critical case: a label that reads as a standing marker would,
    // once spliced into the persisted `goal.description`, silently reclassify this
    // one-off coverage slice as a perpetual standing goal that never completes.
    // It must be rejected — routing the triage brain to ask the operator instead.
    for smuggled in [
        "[standing] PR #7",
        "makes this a standing goal",
        "PERPETUAL GOAL",
    ] {
        assert!(
            matches!(
                validate_tracking_ref(&bad_field("label", smuggled)),
                Err(CorrectionRejected::InvalidTrackingRef { field: "label", .. })
            ),
            "a label carrying a standing marker ({smuggled:?}) must be rejected"
        );
    }

    // The same fail-closed guard covers the docs-only marker: a label carrying
    // `docs-only` / `documentation-only` would, once spliced into the persisted
    // description, flip the goal to non-self-affecting in the completion gate and
    // skip the deploy-aware done-gate (clause 3) — certifying a Simard-affecting
    // goal complete on mere PR merge without a deploy. It must be rejected.
    for smuggled in [
        "docs-only tracking PR",
        "DOCS-ONLY",
        "documentation-only follow-up",
    ] {
        assert!(
            matches!(
                validate_tracking_ref(&bad_field("label", smuggled)),
                Err(CorrectionRejected::InvalidTrackingRef { field: "label", .. })
            ),
            "a label carrying a docs-only marker ({smuggled:?}) must be rejected"
        );
    }

    // And the guard is enforced end-to-end through the constructor: a malformed
    // tracking ref makes the whole target construction fail, so nothing persists.
    assert!(matches!(
        FirstSliceTarget::new("src/lib.rs", 70, "alice", bad_field("label", "a\nb")),
        Err(CorrectionRejected::InvalidTrackingRef { field: "label", .. })
    ));
}

#[test]
fn cycle_count_survives_simulated_restart_and_continues() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let b = board(vec![goal("alpha", 1, None)]);
    let tracker = NoProgressTracker::new();

    // Run four cycles in the "first" daemon process.
    for n in 1..=4 {
        commit_cycle(root, &b, &tracker, n, &[]).unwrap();
    }
    assert_eq!(load(root).cycle_count, 4);

    // --- Simulate a daemon restart (a deploy): a NEW process LOADS the durable
    // brain state instead of resetting to 0/1. ---
    let after_restart = load(root);
    assert_eq!(
        after_restart.cycle_count, 4,
        "the persisted brain counter must survive the restart"
    );

    // The restarted daemon seeds OodaState from the durable value and runs its
    // first post-restart cycle (seed + 1), then commits it.
    let next = after_restart.cycle_count + 1;
    commit_cycle(root, &b, &tracker, next, &[]).unwrap();

    let reloaded = load(root);
    assert_eq!(
        reloaded.cycle_count, 5,
        "the counter CONTINUES the brain's lived cognition across the restart"
    );
    assert_ne!(
        reloaded.cycle_count, 1,
        "the cycle counter must NOT reset to 1 on daemon restart (the reported bug)"
    );
}

#[test]
fn commit_cycle_is_monotonic_and_never_rewinds() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let b = board(vec![goal("alpha", 1, None)]);
    let tracker = NoProgressTracker::new();

    commit_cycle(root, &b, &tracker, 5, &[]).unwrap();
    assert_eq!(load(root).cycle_count, 5);

    // A stale / lower value (e.g. a racing writer, or a rolled-back OodaState)
    // must never rewind the durable counter.
    commit_cycle(root, &b, &tracker, 3, &[]).unwrap();
    assert_eq!(
        load(root).cycle_count,
        5,
        "a lower cycle_count must not rewind the monotonic brain counter"
    );

    // A higher value advances it.
    commit_cycle(root, &b, &tracker, 6, &[]).unwrap();
    assert_eq!(load(root).cycle_count, 6);
}

#[test]
fn legacy_file_without_cycle_count_loads_as_zero_and_self_heals() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // A goal_board.json written by an OLDER daemon build has no `cycle_count`
    // field. Every field is `#[serde(default)]`, so it must still deserialise —
    // with `cycle_count == 0` — rather than failing the load.
    let path = store_path(root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"version":1}"#).unwrap();

    let legacy = load(root);
    assert_eq!(
        legacy.cycle_count, 0,
        "a legacy file lacking the field must load cycle_count as the serde default 0"
    );

    // The next commit re-stamps the field (self-healing; no migration code).
    let b = board(vec![goal("alpha", 1, None)]);
    commit_cycle(root, &b, &NoProgressTracker::new(), 1, &[]).unwrap();
    assert_eq!(
        load(root).cycle_count,
        1,
        "the next commit must re-stamp the durable cycle_count"
    );
}
