use std::path::Path;

use super::local_leaderboard::build_local_leaderboard;
use super::profiles::{PersistedRun, ensure_profile, save_run};
use super::target_loader::TargetSet;
use super::types::{Outcome, OutcomeCode, RunReport, Strategy, TargetFamily};

/// Build a persisted run with an explicit outcome matrix so tests can pin
/// reach/precision without going through the executor.
fn run_with(
    run_id: &str,
    model: &str,
    strategy: Strategy,
    snapshot: &str,
    started: u128,
    offline: bool,
    codes: &[OutcomeCode],
) -> PersistedRun {
    let outcomes = codes
        .iter()
        .enumerate()
        .map(|(i, code)| Outcome {
            target_id: format!("t{i}"),
            family: TargetFamily::NonTrivialReachable,
            code: *code,
            cost_usd: 0.0,
        })
        .collect();
    PersistedRun {
        report: RunReport {
            run_id: run_id.to_string(),
            model: model.to_string(),
            strategy,
            snapshot: snapshot.to_string(),
            started_at_unix_ms: started,
            outcomes,
            offline_scaffold: offline,
        },
        targets: TargetSet {
            snapshot: snapshot.to_string(),
            pinned: Vec::new(),
            held_out_fresh: Vec::new(),
        },
        offline: Default::default(),
    }
}

fn persist(home: &Path, profile: &str, model: &str, run: &PersistedRun) {
    ensure_profile(home, profile, model).unwrap();
    save_run(home, profile, run).unwrap();
}

#[test]
fn empty_gym_reports_no_runs() {
    let dir = tempfile::tempdir().unwrap();
    let board = build_local_leaderboard(dir.path(), None).unwrap();
    assert!(board.rows.is_empty());
    assert!(board.head_to_head.is_empty());
    assert!(!board.multiagent_beats_baseline);
    assert!(board.summary.contains("no local runs"));
}

#[test]
fn ranks_by_reach_then_precision() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // reach 1/2 = 50%, precision 1/2 = 50%
    persist(
        home,
        "a",
        "m",
        &run_with(
            "low",
            "m",
            Strategy::Baseline,
            "snap@v1",
            1,
            false,
            &[OutcomeCode::Reached, OutcomeCode::WrongInput],
        ),
    );
    // reach 2/2 = 100%, precision 2/2 = 100%
    persist(
        home,
        "b",
        "m2",
        &run_with(
            "high",
            "m2",
            Strategy::Baseline,
            "snap@v1",
            2,
            false,
            &[OutcomeCode::Reached, OutcomeCode::Reached],
        ),
    );

    let board = build_local_leaderboard(home, None).unwrap();
    assert_eq!(board.rows.len(), 2);
    assert_eq!(board.rows[0].run_id, "high");
    assert_eq!(board.rows[0].rank, 1);
    assert_eq!(board.rows[1].run_id, "low");
    assert_eq!(board.rows[1].rank, 2);
}

#[test]
fn team_beats_baseline_on_equal_reach_higher_precision() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // Baseline: reach 1/2 = 50%, precision 1/2 = 50% (one wrong submission).
    persist(
        home,
        "base",
        "opus",
        &run_with(
            "b1",
            "opus",
            Strategy::Baseline,
            "snap@v1",
            1,
            false,
            &[OutcomeCode::Reached, OutcomeCode::WrongInput],
        ),
    );
    // Team: reach 1/2 = 50%, precision 1/1 = 100% (abstained on the hard one).
    persist(
        home,
        "team",
        "opus",
        &run_with(
            "t1",
            "opus",
            Strategy::Team,
            "snap@v1",
            2,
            false,
            &[OutcomeCode::Reached, OutcomeCode::Abstained],
        ),
    );

    let board = build_local_leaderboard(home, None).unwrap();
    assert_eq!(board.head_to_head.len(), 1);
    let h = &board.head_to_head[0];
    assert_eq!(h.model, "opus");
    assert!(h.team_beats_baseline);
    assert!(!h.baseline_beats_team);
    assert!(!h.cross_snapshot);
    assert!((h.precision_delta_pct - 50.0).abs() < 1e-9);
    assert!(board.multiagent_beats_baseline);
    assert!(board.summary.contains("MULTIAGENT BEATS"));
}

#[test]
fn baseline_regression_vetoes_aggregate_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // Team reaches fewer targets than baseline ⇒ a regression.
    persist(
        home,
        "base",
        "m",
        &run_with(
            "b",
            "m",
            Strategy::Baseline,
            "snap@v1",
            1,
            false,
            &[OutcomeCode::Reached, OutcomeCode::Reached],
        ),
    );
    persist(
        home,
        "team",
        "m",
        &run_with(
            "t",
            "m",
            Strategy::Team,
            "snap@v1",
            2,
            false,
            &[OutcomeCode::Reached, OutcomeCode::NoSubmission],
        ),
    );

    let board = build_local_leaderboard(home, None).unwrap();
    let h = &board.head_to_head[0];
    assert!(h.baseline_beats_team);
    assert!(!h.team_beats_baseline);
    assert!(!board.multiagent_beats_baseline);
    assert!(board.summary.contains("regression"));
}

#[test]
fn cross_snapshot_pair_is_excluded_from_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    persist(
        home,
        "base",
        "m",
        &run_with(
            "b",
            "m",
            Strategy::Baseline,
            "snap@v1",
            1,
            false,
            &[OutcomeCode::Reached, OutcomeCode::WrongInput],
        ),
    );
    // Team looks better, but on a DIFFERENT snapshot ⇒ not a fair A/B.
    persist(
        home,
        "team",
        "m",
        &run_with(
            "t",
            "m",
            Strategy::Team,
            "snap@v2",
            2,
            false,
            &[OutcomeCode::Reached, OutcomeCode::Reached],
        ),
    );

    let board = build_local_leaderboard(home, None).unwrap();
    let h = &board.head_to_head[0];
    assert!(h.cross_snapshot);
    assert!(!board.multiagent_beats_baseline);
    assert!(board.summary.contains("no comparable"));
    assert!(h.verdict.contains("cross-snapshot"));
}

#[test]
fn profile_filter_narrows_the_ranking() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    persist(
        home,
        "keep",
        "m",
        &run_with(
            "k",
            "m",
            Strategy::Baseline,
            "s",
            1,
            false,
            &[OutcomeCode::Reached],
        ),
    );
    persist(
        home,
        "drop",
        "m2",
        &run_with(
            "d",
            "m2",
            Strategy::Team,
            "s",
            2,
            false,
            &[OutcomeCode::Reached],
        ),
    );

    let board = build_local_leaderboard(home, Some("keep")).unwrap();
    assert_eq!(board.rows.len(), 1);
    assert_eq!(board.rows[0].profile, "keep");
}

#[test]
fn best_run_per_strategy_is_used_in_head_to_head() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // Two team runs; the better one (100% reach) must be the one compared.
    persist(
        home,
        "team",
        "m",
        &run_with(
            "t-weak",
            "m",
            Strategy::Team,
            "snap@v1",
            1,
            false,
            &[OutcomeCode::Reached, OutcomeCode::NoSubmission],
        ),
    );
    persist(
        home,
        "team",
        "m",
        &run_with(
            "t-strong",
            "m",
            Strategy::Team,
            "snap@v1",
            2,
            false,
            &[OutcomeCode::Reached, OutcomeCode::Reached],
        ),
    );
    persist(
        home,
        "base",
        "m",
        &run_with(
            "b",
            "m",
            Strategy::Baseline,
            "snap@v1",
            3,
            false,
            &[OutcomeCode::Reached, OutcomeCode::WrongInput],
        ),
    );

    let board = build_local_leaderboard(home, None).unwrap();
    let h = &board.head_to_head[0];
    assert_eq!(h.team.run_id, "t-strong");
    assert!(h.team_beats_baseline);
}

#[test]
fn offline_scaffold_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    persist(
        home,
        "a",
        "m",
        &run_with(
            "r",
            "m",
            Strategy::Baseline,
            "s",
            1,
            true,
            &[OutcomeCode::Reached],
        ),
    );
    let board = build_local_leaderboard(home, None).unwrap();
    assert!(board.any_offline);
    assert!(board.rows[0].offline_scaffold);
}
