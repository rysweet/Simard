//! Parity tests for the `simard-ooda-step` helper bin (Phase 3 of the
//! recipes-first Simard rebuild — issue #1270).
//!
//! For each deterministic OODA phase we exercise here, we assert that
//! invoking the Rust function directly produces the same output as
//! shelling to the helper bin via JSON IPC. This proves the recipe path
//! and the in-process path are interchangeable.
//!
//! Bridge-dependent phases (observe, act, budget-check, memory-intake,
//! prepare-context) are NOT covered here — they require live bridges
//! and are exercised via integration tests against `run_ooda_cycle`.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use simard::memory_cognitive::CognitiveStatistics;
use simard::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
use simard::improvements::ImprovementDirective;
use simard::ooda_loop::{
    decide, orient, review_outcomes, ActionKind, ActionOutcome, EnvironmentSnapshot, Observation,
    OodaConfig, OodaStateSnapshot, PlannedAction, Priority,
};
use simard::ooda_loop::{OodaPhase, OodaState};

/// Path to the helper binary built by `cargo test`. Cargo sets
/// `CARGO_BIN_EXE_simard-ooda-step` for any test target in the same package.
fn helper_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_simard-ooda-step"))
}

fn write_json<T: serde::Serialize>(dir: &Path, name: &str, value: &T) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, serde_json::to_string(value).expect("serialize")).expect("write");
    p
}

fn run_bin(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(helper_bin()).args(args).output().expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn empty_observation() -> Observation {
    Observation {
        goal_statuses: vec![],
        gym_health: None,
        memory_stats: CognitiveStatistics::default(),
        pending_improvements: vec![],
        environment: EnvironmentSnapshot::default(),
        eval_watchdog: None,
    }
}

fn snapshot_with_goals(goals: Vec<ActiveGoal>) -> OodaStateSnapshot {
    let mut board = GoalBoard::new();
    board.active = goals;
    let state = OodaState::new(board);
    OodaStateSnapshot::from(&state)
}

fn make_goal(id: &str, status: GoalProgress) -> ActiveGoal {
    ActiveGoal {
        id: id.to_string(),
        description: format!("desc-{id}"),
        priority: 1,
        status,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    }
}

// --- orient parity --------------------------------------------------------

#[test]
fn parity_orient_with_blocked_goal() {
    let snapshot =
        snapshot_with_goals(vec![make_goal("g1", GoalProgress::Blocked("waiting".into()))]);
    let observation = empty_observation();

    let direct = orient(
        &observation,
        &snapshot.active_goals,
        &snapshot.goal_failure_counts,
    )
    .expect("orient direct");

    let tmp = tempfile::tempdir().expect("tempdir");
    let snap_path = write_json(tmp.path(), "snap.json", &snapshot);
    let obs_path = write_json(tmp.path(), "obs.json", &observation);

    let (code, stdout, stderr) = run_bin(&[
        "orient",
        "--state-json",
        snap_path.to_str().unwrap(),
        "--observation-json",
        obs_path.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "ooda-step orient failed: stderr={stderr}");
    let recipe: Vec<Priority> = serde_json::from_str(stdout.trim()).expect("parse priorities");

    assert_eq!(recipe.len(), direct.len());
    for (r, d) in recipe.iter().zip(direct.iter()) {
        assert_eq!(r.goal_id, d.goal_id);
        assert!((r.urgency - d.urgency).abs() < 1e-9);
        assert_eq!(r.reason, d.reason);
    }
}

#[test]
fn parity_orient_empty_board_emits_empty_priorities() {
    let snapshot = snapshot_with_goals(vec![]);
    let observation = empty_observation();

    let direct = orient(
        &observation,
        &snapshot.active_goals,
        &snapshot.goal_failure_counts,
    )
    .expect("orient direct");
    assert!(direct.is_empty());

    let tmp = tempfile::tempdir().expect("tempdir");
    let snap_path = write_json(tmp.path(), "snap.json", &snapshot);
    let obs_path = write_json(tmp.path(), "obs.json", &observation);

    let (code, stdout, _) = run_bin(&[
        "orient",
        "--state-json",
        snap_path.to_str().unwrap(),
        "--observation-json",
        obs_path.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    let recipe: Vec<Priority> = serde_json::from_str(stdout.trim()).expect("parse");
    assert!(recipe.is_empty());
}

// --- decide parity --------------------------------------------------------

#[test]
fn parity_decide_caps_at_max_concurrent_actions() {
    let priorities: Vec<Priority> = (0..5)
        .map(|i| Priority {
            goal_id: format!("g{i}"),
            urgency: 0.5,
            reason: "test".into(),
        })
        .collect();
    let mut config = OodaConfig::default();
    config.max_concurrent_actions = 2;

    let direct = decide(&priorities, &config).expect("decide direct");
    assert_eq!(direct.len(), 2);

    let tmp = tempfile::tempdir().expect("tempdir");
    let pri_path = write_json(tmp.path(), "pri.json", &priorities);
    let cfg_path = write_json(tmp.path(), "cfg.json", &config);

    let (code, stdout, stderr) = run_bin(&[
        "decide",
        "--priorities-json",
        pri_path.to_str().unwrap(),
        "--config-json",
        cfg_path.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "decide failed: {stderr}");
    let recipe: Vec<PlannedAction> = serde_json::from_str(stdout.trim()).expect("parse actions");

    assert_eq!(recipe.len(), direct.len());
    for (r, d) in recipe.iter().zip(direct.iter()) {
        assert_eq!(r.goal_id, d.goal_id);
        assert_eq!(r.kind, d.kind);
        assert_eq!(r.description, d.description);
    }
}

#[test]
fn parity_decide_skips_zero_urgency() {
    let priorities = vec![
        Priority {
            goal_id: "g1".into(),
            urgency: 0.0,
            reason: "".into(),
        },
        Priority {
            goal_id: "g2".into(),
            urgency: 0.7,
            reason: "ok".into(),
        },
    ];
    let config = OodaConfig::default();
    let direct = decide(&priorities, &config).expect("decide direct");
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].goal_id.as_deref(), Some("g2"));

    let tmp = tempfile::tempdir().expect("tempdir");
    let pri_path = write_json(tmp.path(), "pri.json", &priorities);
    let (code, stdout, _) = run_bin(&[
        "decide",
        "--priorities-json",
        pri_path.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    let recipe: Vec<PlannedAction> = serde_json::from_str(stdout.trim()).expect("parse");
    assert_eq!(recipe.len(), 1);
    assert_eq!(recipe[0].goal_id.as_deref(), Some("g2"));
}

// --- review parity --------------------------------------------------------

#[test]
fn parity_review_failed_outcome_yields_fix_directive() {
    let outcomes = vec![ActionOutcome {
        action: PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance g1".into(),
        },
        success: false,
        detail: "ran into error X".into(),
    }];

    let direct = review_outcomes(&outcomes, Duration::from_millis(500));
    assert_eq!(direct.len(), 1);

    let tmp = tempfile::tempdir().expect("tempdir");
    let out_path = write_json(tmp.path(), "out.json", &outcomes);
    let (code, stdout, stderr) = run_bin(&[
        "review",
        "--outcomes-json",
        out_path.to_str().unwrap(),
        "--act-elapsed-millis",
        "500",
    ]);
    assert_eq!(code, 0, "review failed: {stderr}");
    let recipe: Vec<ImprovementDirective> = serde_json::from_str(stdout.trim()).expect("parse");

    assert_eq!(recipe.len(), direct.len());
    assert_eq!(recipe[0], direct[0]);
}

#[test]
fn parity_review_no_outcomes_yields_no_directives() {
    let outcomes: Vec<ActionOutcome> = vec![];
    let direct = review_outcomes(&outcomes, Duration::from_millis(0));
    assert!(direct.is_empty());

    let tmp = tempfile::tempdir().expect("tempdir");
    let out_path = write_json(tmp.path(), "out.json", &outcomes);
    let (code, stdout, _) = run_bin(&[
        "review",
        "--outcomes-json",
        out_path.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    let recipe: Vec<ImprovementDirective> = serde_json::from_str(stdout.trim()).expect("parse");
    assert!(recipe.is_empty());
}

// --- curate parity --------------------------------------------------------

#[test]
fn parity_curate_archives_completed_goals() {
    let snapshot = snapshot_with_goals(vec![
        make_goal("g-active", GoalProgress::InProgress { percent: 50 }),
        make_goal("g-done", GoalProgress::Completed),
    ]);

    // Direct: import the goal_curation function ourselves
    let mut state_direct = snapshot.clone().into_state();
    let archived_direct =
        simard::goal_curation::archive_completed(&mut state_direct.active_goals);
    let archived_direct_ids: Vec<String> =
        archived_direct.iter().map(|g| g.id.clone()).collect();
    assert_eq!(archived_direct_ids, vec!["g-done"]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let snap_path = write_json(tmp.path(), "snap.json", &snapshot);

    let (code, stdout, stderr) = run_bin(&[
        "curate",
        "--state-json",
        snap_path.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "curate failed: {stderr}");
    let result: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse");
    let archived_recipe: Vec<String> =
        serde_json::from_value(result["archived_goal_ids"].clone()).expect("archived");
    assert_eq!(archived_recipe, archived_direct_ids);

    // Snapshot should still contain the active goal
    let snap_back: OodaStateSnapshot =
        serde_json::from_value(result["snapshot"].clone()).expect("snap");
    assert_eq!(snap_back.active_goals.active.len(), 1);
    assert_eq!(snap_back.active_goals.active[0].id, "g-active");
}

// --- act subcommand surface ----------------------------------------------

#[test]
fn act_rejects_missing_state_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let actions_path = write_json::<Vec<PlannedAction>>(tmp.path(), "actions.json", &vec![]);
    let (code, _stdout, stderr) = run_bin(&[
        "act",
        "--actions-json",
        actions_path.to_str().unwrap(),
        "--state-root",
        tmp.path().to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("json envelope");
    assert!(
        parsed["error"]
            .as_str()
            .unwrap_or("")
            .contains("state-json"),
        "expected --state-json error: {parsed}"
    );
}

#[test]
fn act_rejects_missing_actions_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let snap = snapshot_with_goals(vec![]);
    let snap_path = write_json(tmp.path(), "snap.json", &snap);
    let (code, _stdout, stderr) = run_bin(&[
        "act",
        "--state-json",
        snap_path.to_str().unwrap(),
        "--state-root",
        tmp.path().to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("json envelope");
    assert!(
        parsed["error"]
            .as_str()
            .unwrap_or("")
            .contains("actions-json"),
        "expected --actions-json error: {parsed}"
    );
}

// --- observe subcommand surface (live execution requires bridges and is
//      exercised by the daemon path; here we only verify the helper bin
//      surface accepts the right flags and rejects malformed invocations) -

#[test]
fn observe_rejects_missing_state_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (code, _stdout, stderr) = run_bin(&[
        "observe",
        "--state-root",
        tmp.path().to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("json envelope");
    assert!(
        parsed["error"]
            .as_str()
            .unwrap_or("")
            .contains("state-json"),
        "expected --state-json error: {parsed}"
    );
}

#[test]
fn observe_rejects_missing_state_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let snap = snapshot_with_goals(vec![]);
    let snap_path = write_json(tmp.path(), "snap.json", &snap);
    let (code, _stdout, stderr) = run_bin(&[
        "observe",
        "--state-json",
        snap_path.to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("json envelope");
    assert!(
        parsed["error"]
            .as_str()
            .unwrap_or("")
            .contains("state-root"),
        "expected --state-root error: {parsed}"
    );
}

// --- error envelope -------------------------------------------------------

#[test]
fn errors_use_json_envelope_on_stderr() {
    let (code, _stdout, stderr) = run_bin(&["nope"]);
    assert_eq!(code, 2);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("json envelope");
    assert!(parsed["error"].is_string());
}

#[test]
fn missing_required_flag_yields_error_envelope() {
    let (code, _stdout, stderr) = run_bin(&["orient"]);
    assert_eq!(code, 2);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("json envelope");
    assert!(
        parsed["error"]
            .as_str()
            .unwrap_or("")
            .contains("state-json"),
        "expected error to mention missing --state-json: {parsed}"
    );
}

// silence unused_imports warning for OodaPhase (it's used implicitly via
// OodaStateSnapshot serde)
#[allow(dead_code)]
fn _ensure_ooda_phase_in_scope(_: OodaPhase) {}
