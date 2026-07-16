//! Tests for the baseline-vs-team A/B comparator ([`super::ab`]) and its
//! `coin-gym ab` CLI command.

use super::ab::{StrategyComparison, StrategyVerdict, compare_strategies};
use super::profiles::runs_dir;
use super::target_loader::DemoScenario;
use super::{dispatch_with_home, execute_run};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn compare_strategies_scores_team_win_on_bundled_sample() {
    let scenario = DemoScenario::sample().unwrap();
    let cmp = compare_strategies("claude-opus-4.6", &scenario).unwrap();

    // The reference measurement: equal reach (3/5), team lifts precision from
    // 60% to 100% by abstaining on the two low-confidence (wrong) submissions.
    assert!(approx(cmp.baseline.overall.reach_pct(), 60.0));
    assert!(approx(cmp.baseline.overall.precision_pct(), 60.0));
    assert!(approx(cmp.team.overall.reach_pct(), 60.0));
    assert!(approx(cmp.team.overall.precision_pct(), 100.0));

    assert!(approx(cmp.reach_delta_pct, 0.0));
    assert!(approx(cmp.precision_delta_pct, 40.0));

    // Team ties reach and strictly wins precision ⇒ Pareto domination.
    assert_eq!(cmp.verdict, StrategyVerdict::TeamWins);
    assert!(cmp.offline_scaffold);
    assert!(cmp.note.contains("dominates"));
    assert!(cmp.note.contains("LOCAL-ONLY"));
}

#[test]
fn from_reports_reads_model_and_snapshot_from_baseline_arm() {
    let scenario = DemoScenario::sample().unwrap();
    let baseline = execute_run(
        "claude-opus-4.6",
        super::types::Strategy::Baseline,
        &scenario,
    )
    .unwrap();
    let team = execute_run("claude-opus-4.6", super::types::Strategy::Team, &scenario).unwrap();
    let cmp = StrategyComparison::from_reports(&baseline, &team);
    assert_eq!(cmp.model, "claude-opus-4.6");
    assert_eq!(cmp.snapshot, baseline.snapshot);
}

#[test]
fn verdict_classify_covers_all_quadrants() {
    // team strictly better on both ⇒ team wins.
    assert_eq!(
        StrategyVerdict::classify(5.0, 5.0),
        StrategyVerdict::TeamWins
    );
    // team ties reach, wins precision ⇒ team wins.
    assert_eq!(
        StrategyVerdict::classify(0.0, 5.0),
        StrategyVerdict::TeamWins
    );
    // baseline strictly better on both ⇒ baseline wins.
    assert_eq!(
        StrategyVerdict::classify(-5.0, -5.0),
        StrategyVerdict::BaselineWins
    );
    // baseline ties reach, wins precision ⇒ baseline wins.
    assert_eq!(
        StrategyVerdict::classify(0.0, -5.0),
        StrategyVerdict::BaselineWins
    );
    // equal on both ⇒ tie.
    assert_eq!(StrategyVerdict::classify(0.0, 0.0), StrategyVerdict::Tie);
    // one up, one down ⇒ genuine trade-off.
    assert_eq!(StrategyVerdict::classify(5.0, -5.0), StrategyVerdict::Mixed);
    assert_eq!(StrategyVerdict::classify(-5.0, 5.0), StrategyVerdict::Mixed);
}

#[test]
fn verdict_classify_treats_fp_jitter_as_equal() {
    // Sub-epsilon deltas must not tip the verdict away from a tie.
    assert_eq!(
        StrategyVerdict::classify(1e-12, -1e-12),
        StrategyVerdict::Tie
    );
}

#[test]
fn ab_command_persists_both_arms_under_one_profile() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    dispatch_with_home(home, args(&["ab", "claude-opus-4.6", "--profile", "opus"])).unwrap();

    let runs = runs_dir(home, "opus");
    let entries: Vec<_> = std::fs::read_dir(&runs)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();
    assert_eq!(
        entries.len(),
        2,
        "ab writes exactly one run file per arm (baseline + team)"
    );

    // One file per strategy label.
    let names: Vec<String> = entries
        .iter()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n.contains("-baseline-")),
        "a baseline run file should be present: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.contains("-team-")),
        "a team run file should be present: {names:?}"
    );
}

#[test]
fn ab_command_requires_a_model() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_with_home(dir.path(), args(&["ab"])).unwrap_err();
    assert!(err.to_string().contains("expected <model>"));
}
