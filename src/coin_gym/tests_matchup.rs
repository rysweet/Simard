use super::matchup::{MatchupVerdict, decide_matchup};
use super::scorer::score_run;
use super::target_loader::DemoScenario;
use super::types::{Outcome, OutcomeCode, RunReport, Strategy, TargetFamily};
use super::{dispatch_with_home, execute_run};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Build a synthetic single-outcome report so verdict logic can be tested
/// independently of the fixture oracle.
fn report_with(run_id: &str, code: OutcomeCode) -> RunReport {
    RunReport {
        run_id: run_id.to_string(),
        model: "m".to_string(),
        strategy: Strategy::Baseline,
        snapshot: "s".to_string(),
        started_at_unix_ms: 0,
        outcomes: vec![Outcome {
            target_id: "t1".to_string(),
            family: TargetFamily::Frontier,
            code,
            cost_usd: 0.0,
        }],
        offline_scaffold: true,
    }
}

#[test]
fn team_wins_when_reach_is_higher() {
    // Baseline abstains (reach 0%); team reaches (reach 100%).
    let baseline = score_run(&report_with("b", OutcomeCode::Abstained));
    let team = score_run(&report_with("t", OutcomeCode::Reached));
    let m = decide_matchup(&baseline, &team);
    assert_eq!(m.verdict, MatchupVerdict::TeamWins);
    assert!(approx(m.reach_delta_pp, 100.0));
    assert_eq!(m.baseline_run_id, "b");
    assert_eq!(m.team_run_id, "t");
    assert_eq!(m.targets, 1);
    assert!(m.offline_scaffold);
}

#[test]
fn baseline_wins_when_reach_is_higher() {
    let baseline = score_run(&report_with("b", OutcomeCode::Reached));
    let team = score_run(&report_with("t", OutcomeCode::Abstained));
    let m = decide_matchup(&baseline, &team);
    assert_eq!(m.verdict, MatchupVerdict::BaselineWins);
    assert!(approx(m.reach_delta_pp, -100.0));
}

#[test]
fn precision_breaks_a_reach_tie() {
    // Equal reach (both reach), but the baseline also submitted a wrong input,
    // dragging its precision below the team's — so the team wins on the tie-break.
    let mut baseline_report = report_with("b", OutcomeCode::Reached);
    baseline_report.outcomes.push(Outcome {
        target_id: "t2".to_string(),
        family: TargetFamily::Frontier,
        code: OutcomeCode::WrongInput,
        cost_usd: 0.0,
    });
    let mut team_report = report_with("t", OutcomeCode::Reached);
    team_report.outcomes.push(Outcome {
        target_id: "t2".to_string(),
        family: TargetFamily::Frontier,
        code: OutcomeCode::Abstained,
        cost_usd: 0.0,
    });
    let baseline = score_run(&baseline_report);
    let team = score_run(&team_report);
    let m = decide_matchup(&baseline, &team);
    assert!(approx(m.reach_delta_pp, 0.0), "reach is tied");
    assert!(m.precision_delta_pp > 0.0, "team precision is higher");
    assert_eq!(m.verdict, MatchupVerdict::TeamWins);
}

#[test]
fn identical_scores_are_a_tie() {
    let baseline = score_run(&report_with("b", OutcomeCode::Reached));
    let team = score_run(&report_with("t", OutcomeCode::Reached));
    let m = decide_matchup(&baseline, &team);
    assert_eq!(m.verdict, MatchupVerdict::Tie);
    assert!(approx(m.reach_delta_pp, 0.0));
    assert!(approx(m.precision_delta_pp, 0.0));
}

#[test]
fn sample_scenario_matchup_shows_precision_win_for_team() {
    // On the shipped sample, baseline and team reach the same targets but the
    // team's abstention gate lifts precision — a genuine multiagent advantage.
    let scenario = DemoScenario::sample().unwrap();
    let baseline = score_run(&execute_run("m", Strategy::Baseline, &scenario).unwrap());
    let team = score_run(&execute_run("m", Strategy::Team, &scenario).unwrap());
    let m = decide_matchup(&baseline, &team);
    assert!(approx(m.reach_delta_pp, 0.0));
    assert!(m.precision_delta_pp > 0.0);
    assert_eq!(m.verdict, MatchupVerdict::TeamWins);
}

#[test]
fn matchup_command_persists_both_runs() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    dispatch_with_home(
        home,
        args(&["matchup", "claude-opus-4.6", "--profile", "duel"]),
    )
    .unwrap();

    let runs = super::profiles::runs_dir(home, "duel");
    let mut kinds: Vec<String> = std::fs::read_dir(&runs)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .map(|e| e.path().file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    kinds.sort();
    assert_eq!(kinds.len(), 2, "matchup persists a baseline and a team run");
    assert!(
        kinds.iter().any(|k| k.contains("baseline")),
        "one run is the baseline: {kinds:?}"
    );
    assert!(
        kinds.iter().any(|k| k.contains("team")),
        "one run is the team: {kinds:?}"
    );
}

#[test]
fn matchup_requires_a_model() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_with_home(dir.path(), args(&["matchup"])).unwrap_err();
    assert!(err.to_string().contains("expected <model>"));
}

#[test]
fn matchup_rejects_unknown_flag() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_with_home(dir.path(), args(&["matchup", "m", "--bogus", "x"])).unwrap_err();
    assert!(err.to_string().contains("unknown flag"));
}
