use super::duel::{DuelVerdict, decide};
use super::target_loader::DemoScenario;
use super::types::{Outcome, OutcomeCode, RunReport, Strategy, TargetFamily};
use super::{dispatch_with_home, execute_run};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Build a synthetic run report from a list of per-target outcome codes so the
/// verdict ordering can be exercised without a scenario.
fn report(strategy: Strategy, codes: &[OutcomeCode]) -> RunReport {
    let outcomes = codes
        .iter()
        .enumerate()
        .map(|(i, &code)| Outcome {
            target_id: format!("t{i}"),
            family: TargetFamily::Frontier,
            code,
            cost_usd: 0.0,
        })
        .collect();
    RunReport {
        run_id: format!("m-{}-0-0", strategy.label()),
        model: "m".to_string(),
        strategy,
        snapshot: "snap".to_string(),
        started_at_unix_ms: 0,
        outcomes,
        offline_scaffold: true,
    }
}

#[test]
fn sample_duel_team_wins_on_precision_at_equal_reach() {
    let scenario = DemoScenario::sample().unwrap();
    let baseline = execute_run("claude-opus-4.6", Strategy::Baseline, &scenario).unwrap();
    let team = execute_run("claude-opus-4.6", Strategy::Team, &scenario).unwrap();

    let d = decide(&baseline, &team);

    assert_eq!(d.verdict, DuelVerdict::TeamWins);
    assert!(d.verdict.team_wins());
    assert!(
        approx(d.reach_delta_pct, 0.0),
        "reach ties on the sample set"
    );
    assert!(
        approx(d.precision_delta_pct, 40.0),
        "team's abstention gate lifts precision +40 pts, got {}",
        d.precision_delta_pct
    );
    // Arms are labelled and traceable back to their persisted run ids.
    assert_eq!(d.baseline.strategy, Strategy::Baseline);
    assert_eq!(d.team.strategy, Strategy::Team);
    assert_eq!(d.baseline.run_id, baseline.run_id);
    assert_eq!(d.team.run_id, team.run_id);
    assert!(d.reason.contains("precision"));
    assert!(d.offline_scaffold);
    assert_eq!(d.snapshot, baseline.snapshot);
    assert_eq!(d.targets, 5);
}

#[test]
fn team_wins_when_reach_is_strictly_higher() {
    use OutcomeCode::{NoSubmission, Reached};
    // baseline reaches 1/2 (50%); team reaches 2/2 (100%).
    let baseline = report(Strategy::Baseline, &[Reached, NoSubmission]);
    let team = report(Strategy::Team, &[Reached, Reached]);
    let d = decide(&baseline, &team);
    assert_eq!(d.verdict, DuelVerdict::TeamWins);
    assert!(d.reach_delta_pct > 0.0);
    assert!(d.reason.contains("reach"));
}

#[test]
fn baseline_wins_when_reach_is_strictly_higher() {
    use OutcomeCode::{Abstained, Reached};
    // baseline reaches 2/2 (100%); team abstains one → reaches 1/2 (50%).
    let baseline = report(Strategy::Baseline, &[Reached, Reached]);
    let team = report(Strategy::Team, &[Reached, Abstained]);
    let d = decide(&baseline, &team);
    assert_eq!(d.verdict, DuelVerdict::BaselineWins);
    assert!(d.reach_delta_pct < 0.0);
    assert!(d.reason.contains("baseline"));
}

#[test]
fn team_wins_on_precision_when_reach_ties() {
    use OutcomeCode::{Abstained, Reached, WrongInput};
    // Equal reach (1/2); baseline over-claims (W) → 50% precision, team abstains
    // → 100% precision.
    let baseline = report(Strategy::Baseline, &[Reached, WrongInput]);
    let team = report(Strategy::Team, &[Reached, Abstained]);
    let d = decide(&baseline, &team);
    assert_eq!(d.verdict, DuelVerdict::TeamWins);
    assert!(approx(d.reach_delta_pct, 0.0));
    assert!(d.precision_delta_pct > 0.0);
}

#[test]
fn baseline_wins_on_precision_when_reach_ties() {
    use OutcomeCode::{Abstained, Reached, WrongInput};
    // Equal reach (1/2); baseline abstains (100% precision), team over-claims.
    let baseline = report(Strategy::Baseline, &[Reached, Abstained]);
    let team = report(Strategy::Team, &[Reached, WrongInput]);
    let d = decide(&baseline, &team);
    assert_eq!(d.verdict, DuelVerdict::BaselineWins);
    assert!(approx(d.reach_delta_pct, 0.0));
    assert!(d.precision_delta_pct < 0.0);
}

#[test]
fn identical_arms_are_a_tie() {
    use OutcomeCode::{Abstained, Reached};
    let baseline = report(Strategy::Baseline, &[Reached, Abstained]);
    let team = report(Strategy::Team, &[Reached, Abstained]);
    let d = decide(&baseline, &team);
    assert_eq!(d.verdict, DuelVerdict::Tie);
    assert!(approx(d.reach_delta_pct, 0.0));
    assert!(approx(d.precision_delta_pct, 0.0));
    assert!(!d.verdict.team_wins());
}

#[test]
fn verdict_serialises_to_kebab_case() {
    let json = serde_json::to_string(&DuelVerdict::TeamWins).unwrap();
    assert_eq!(json, "\"team-wins\"");
}

#[test]
fn duel_command_persists_both_arms_under_one_profile() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    dispatch_with_home(
        home,
        args(&["duel", "claude-opus-4.6", "--profile", "opus"]),
    )
    .unwrap();

    let runs = super::profiles::runs_dir(home, "opus");
    let entries: Vec<_> = std::fs::read_dir(&runs)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();
    assert_eq!(entries.len(), 2, "duel writes a baseline and a team run");
}

#[test]
fn duel_requires_model() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_with_home(dir.path(), args(&["duel"])).unwrap_err();
    assert!(err.to_string().contains("expected <model>"));
}

#[test]
fn duel_defaults_profile_from_model() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // No --profile: the profile is derived from the sanitised model name, and
    // both arms land in it (a model-derived profile is per-model, so baseline +
    // team share it).
    dispatch_with_home(home, args(&["duel", "claude-opus-4.6"])).unwrap();
    let profiles = super::profiles::list_profiles(home).unwrap();
    assert_eq!(profiles.len(), 1);
    let runs = super::profiles::runs_dir(home, &profiles[0].name);
    let count = std::fs::read_dir(&runs)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .count();
    assert_eq!(count, 2);
}
