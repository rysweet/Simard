use super::execute_run;
use super::improve_loop::{
    SliceMeasurement, TacticDecision, improves_holdout, load_tactic_memory, run_self_improvement,
    save_tactic_memory,
};
use super::profiles::PersistedRun;
use super::target_loader::{DemoScenario, OfflineScaffold, TargetSet};
use super::types::Strategy;

/// The Phase-5 loop fixture: pinned decoder+crypto+generic failures; held-out
/// fresh covers decoder+crypto but NOT generic.
const LOOP_SNAPSHOT: &str = include_str!("fixtures/improve_loop_snapshot.json");

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// Build a persisted offline scaffold run (baseline on pinned) from the loop
/// fixture, ready for `run_self_improvement`.
fn persisted_loop_run() -> PersistedRun {
    let scenario = DemoScenario::from_manifest(LOOP_SNAPSHOT).unwrap();
    let report = execute_run("claude-opus-4.6", Strategy::Baseline, &scenario).unwrap();
    PersistedRun {
        report,
        targets: scenario.targets.clone(),
        offline: scenario.offline_scaffold(),
    }
}

#[test]
fn full_cycle_keeps_generalising_and_rolls_back_overfit() {
    let home = tempfile::tempdir().unwrap();
    let persisted = persisted_loop_run();

    let report = run_self_improvement(home.path(), "loop", &persisted).unwrap();

    // All three analyst tactics are general → accepted by the gate.
    assert_eq!(report.gate_accepted, 3);
    assert_eq!(report.gate_rejected, 0);

    // Decoder + crypto generalise to held-out fresh → kept; generic does not.
    assert_eq!(report.kept, 2, "decoder + crypto tactics generalise");
    assert_eq!(report.rolled_back, 1, "generic tactic rolls back");
    assert_eq!(report.overfitting_warnings, 1);

    // Held-out reach lifts from 0% (fresh lines unsolved) to 100% after banking.
    assert!(approx(report.holdout_reach_before_pct, 0.0));
    assert!(approx(report.holdout_reach_after_pct, 100.0));

    // Durable memory grows by the two kept tactics.
    assert_eq!(report.memory_before, 0);
    assert_eq!(report.memory_after, 2);

    // Per-tactic decisions.
    let dec = report
        .verified
        .iter()
        .find(|v| v.source_target_id == "dec-a")
        .unwrap();
    assert_eq!(dec.decision, TacticDecision::Keep);
    assert_eq!(dec.category, "format-gated-decoder");
    assert!(dec.newly_persisted);
    assert!(dec.overfitting_warning.is_none());

    let cry = report
        .verified
        .iter()
        .find(|v| v.source_target_id == "cry-a")
        .unwrap();
    assert_eq!(cry.decision, TacticDecision::Keep);
    assert_eq!(cry.category, "crypto-state-machine");

    let generic = report
        .verified
        .iter()
        .find(|v| v.source_target_id == "gen-a")
        .unwrap();
    assert_eq!(generic.decision, TacticDecision::Rollback);
    assert_eq!(generic.category, "generic");
    assert!(!generic.newly_persisted);
    // The train/held-out gap fires the "overfitting-warning", worded honestly as
    // a coverage gap (UNPROVEN) rather than a definitive overfit claim.
    let warning = generic.overfitting_warning.as_ref().unwrap();
    assert!(warning.contains("GAP"), "warning: {warning}");
    assert!(warning.contains("UNPROVEN"), "warning: {warning}");
    assert!(generic.train_after.reached > generic.train_before.reached);
    assert_eq!(
        generic.holdout_after.reached,
        generic.holdout_before.reached
    );
}

#[test]
fn kept_tactics_persist_to_memory_and_are_reused_next_run() {
    let home = tempfile::tempdir().unwrap();
    let persisted = persisted_loop_run();

    // First cycle banks decoder + crypto.
    let first = run_self_improvement(home.path(), "loop", &persisted).unwrap();
    assert_eq!(first.kept, 2);

    // The tactics are on disk, keyed by general family (never per project/target).
    let memory = load_tactic_memory(home.path(), "loop").unwrap();
    assert_eq!(memory.tactics.len(), 2);
    let families: Vec<&str> = memory.tactics.iter().map(|t| t.category.as_str()).collect();
    assert!(families.contains(&"format-gated-decoder"));
    assert!(families.contains(&"crypto-state-machine"));
    assert!(
        !families
            .iter()
            .any(|f| f.contains("acme") || f.contains("delta"))
    );

    // Second cycle reuses them: the held-out baseline already reaches 100%, so
    // nothing new is banked and memory does not double-count.
    let second = run_self_improvement(home.path(), "loop", &persisted).unwrap();
    assert_eq!(second.memory_before, 2, "prior cycle's tactics are loaded");
    assert!(
        approx(second.holdout_reach_before_pct, 100.0),
        "reused wins"
    );
    assert_eq!(second.kept, 0, "no new tactics banked on the reuse run");
    assert_eq!(second.memory_after, 2, "memory is not double-banked");
}

#[test]
fn improves_holdout_requires_reach_up_and_no_precision_drop() {
    let m = |reached, submitted, total, reach, precision| SliceMeasurement {
        reached,
        submitted,
        total,
        reach_pct: reach,
        precision_pct: precision,
    };

    // reach up, precision steady → keep.
    assert!(improves_holdout(
        &m(1, 1, 4, 25.0, 100.0),
        &m(2, 2, 4, 50.0, 100.0)
    ));

    // reach up but precision drops (added a wrong submission) → roll back.
    assert!(!improves_holdout(
        &m(2, 2, 4, 50.0, 100.0),
        &m(3, 6, 4, 75.0, 50.0)
    ));

    // reach flat → roll back.
    assert!(!improves_holdout(
        &m(2, 2, 4, 50.0, 100.0),
        &m(2, 3, 4, 50.0, 66.7)
    ));
}

#[test]
fn rejects_real_run_without_offline_scaffold() {
    let home = tempfile::tempdir().unwrap();
    let scenario = DemoScenario::from_manifest(LOOP_SNAPSHOT).unwrap();
    let report = execute_run("m", Strategy::Baseline, &scenario).unwrap();
    // A run with no persisted mock context (as a real `coin verify` run would be).
    let persisted = PersistedRun {
        report,
        targets: scenario.targets.clone(),
        offline: OfflineScaffold::default(),
    };
    let err = run_self_improvement(home.path(), "loop", &persisted).unwrap_err();
    assert!(err.to_string().contains("OFFLINE SCAFFOLD"));
}

#[test]
fn rejects_run_without_a_held_out_slice() {
    let home = tempfile::tempdir().unwrap();
    let mut scenario = DemoScenario::from_manifest(LOOP_SNAPSHOT).unwrap();
    // Drop the held-out slice: there is nothing fresh to verify against.
    scenario.targets = TargetSet {
        snapshot: scenario.targets.snapshot.clone(),
        pinned: scenario.targets.pinned.clone(),
        held_out_fresh: Vec::new(),
    };
    let report = execute_run("m", Strategy::Baseline, &scenario).unwrap();
    let persisted = PersistedRun {
        report,
        targets: scenario.targets.clone(),
        offline: scenario.offline_scaffold(),
    };
    let err = run_self_improvement(home.path(), "loop", &persisted).unwrap_err();
    assert!(err.to_string().contains("held-out fresh slice"));
}

#[test]
fn tactic_memory_round_trips_on_disk() {
    let home = tempfile::tempdir().unwrap();
    let persisted = persisted_loop_run();
    let report = run_self_improvement(home.path(), "loop", &persisted).unwrap();
    assert_eq!(report.memory_after, 2);

    let loaded = load_tactic_memory(home.path(), "loop").unwrap();
    // Re-save then reload is stable.
    save_tactic_memory(home.path(), "loop", &loaded).unwrap();
    let reloaded = load_tactic_memory(home.path(), "loop").unwrap();
    assert_eq!(loaded, reloaded);
}
