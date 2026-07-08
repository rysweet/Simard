use super::profiles::{PersistedRun, ensure_profile, runs_dir, save_run};
use super::target_loader::DemoScenario;
use super::types::Strategy;
use super::{coin_gym_usage, dispatch_with_home, execute_run};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn execute_run_baseline_vs_team_shows_precision_tradeoff() {
    let scenario = DemoScenario::sample().unwrap();
    let baseline = execute_run("claude-opus-4.6", Strategy::Baseline, &scenario).unwrap();
    let team = execute_run("claude-opus-4.6", Strategy::Team, &scenario).unwrap();

    let bscore = super::scorer::score_run(&baseline);
    let tscore = super::scorer::score_run(&team);

    // Same reach on identical targets, but the team's abstention gate lifts
    // precision by declining low-confidence (wrong) submissions.
    assert!(approx(bscore.overall.reach_rate, 0.6));
    assert!(approx(bscore.overall.precision, 0.6));
    assert!(approx(tscore.overall.reach_rate, 0.6));
    assert!(approx(tscore.overall.precision, 1.0));
    assert!(tscore.overall.precision > bscore.overall.precision);

    assert!(baseline.offline_scaffold);
}

#[test]
fn run_command_creates_profile_and_run_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    dispatch_with_home(
        home,
        args(&[
            "run",
            "claude-opus-4.6",
            "--strategy",
            "team",
            "--profile",
            "opus",
        ]),
    )
    .unwrap();

    let runs = runs_dir(home, "opus");
    let entries: Vec<_> = std::fs::read_dir(&runs)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();
    assert_eq!(entries.len(), 1, "exactly one run file should be written");
}

#[test]
fn score_compare_improve_load_a_saved_run() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let scenario = DemoScenario::sample().unwrap();
    let report = execute_run("claude-opus-4.6", Strategy::Baseline, &scenario).unwrap();
    ensure_profile(home, "opus", "claude-opus-4.6").unwrap();
    save_run(
        home,
        "opus",
        &PersistedRun {
            report: report.clone(),
            targets: scenario.targets.clone(),
            offline: scenario.offline_scaffold(),
        },
    )
    .unwrap();

    dispatch_with_home(home, args(&["score", &report.run_id, "--profile", "opus"])).unwrap();
    dispatch_with_home(
        home,
        args(&["compare", &report.run_id, "--profile", "opus"]),
    )
    .unwrap();
    dispatch_with_home(
        home,
        args(&["improve", &report.run_id, "--profile", "opus"]),
    )
    .unwrap();
}

#[test]
fn profiles_command_runs_on_empty_and_populated_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // empty
    dispatch_with_home(home, args(&["profiles"])).unwrap();
    // populated
    ensure_profile(home, "opus", "claude-opus-4.6").unwrap();
    dispatch_with_home(home, args(&["profiles"])).unwrap();
}

#[test]
fn unknown_command_errors_with_usage() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_with_home(dir.path(), args(&["frobnicate"])).unwrap_err();
    assert!(err.to_string().contains("unknown command"));
}

#[test]
fn empty_args_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_with_home(dir.path(), Vec::<String>::new()).unwrap_err();
    assert!(err.to_string().contains("usage"));
}

#[test]
fn run_requires_model() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_with_home(dir.path(), args(&["run"])).unwrap_err();
    assert!(err.to_string().contains("expected <model>"));
}

#[test]
fn unknown_flag_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_with_home(dir.path(), args(&["run", "m", "--bogus", "x"])).unwrap_err();
    assert!(err.to_string().contains("unknown flag"));
}

#[test]
fn invalid_strategy_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let err =
        dispatch_with_home(dir.path(), args(&["run", "m", "--strategy", "solo"])).unwrap_err();
    assert!(err.to_string().contains("unknown strategy"));
}

#[test]
fn score_requires_run_id() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_with_home(dir.path(), args(&["score"])).unwrap_err();
    assert!(err.to_string().contains("expected <run-id>"));
}

#[test]
fn run_id_with_path_traversal_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    for bad in ["../secret", "a/b", "..\\win", ".."] {
        let err = dispatch_with_home(dir.path(), args(&["score", bad])).unwrap_err();
        assert!(
            err.to_string().contains("invalid run-id"),
            "should reject traversal run-id: {bad}"
        );
    }
}

#[test]
fn explicit_profile_name_is_sanitised() {
    // A traversal-y --profile must be confined under <home>/profiles, not escape it.
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    dispatch_with_home(
        home,
        args(&[
            "run",
            "m",
            "--strategy",
            "baseline",
            "--profile",
            "../escape",
        ]),
    )
    .unwrap();
    // No directory was created outside the home tree.
    assert!(!home.parent().unwrap().join("escape").exists());
    // The sanitised profile lives under home/profiles.
    let safe = super::profiles::sanitize_name("../escape");
    assert!(!safe.contains('/') && !safe.contains(".."));
    assert!(home.join("profiles").join(&safe).exists());
}

#[test]
fn usage_lists_all_subcommands() {
    let usage = coin_gym_usage();
    for cmd in ["run", "score", "compare", "improve", "contract", "profiles"] {
        assert!(usage.contains(cmd), "usage should mention {cmd}");
    }
}

fn write_manifest(dir: &std::path::Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn run_rejects_empty_pinned_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = write_manifest(
        dir.path(),
        "empty.json",
        r#"{"snapshot":"s","targets":{"pinned":[],"held_out_fresh":[]}}"#,
    );
    let err =
        dispatch_with_home(dir.path(), args(&["run", "m", "--targets", &manifest])).unwrap_err();
    assert!(err.to_string().contains("no pinned targets"));
}

#[test]
fn run_rejects_manifest_missing_oracle() {
    let dir = tempfile::tempdir().unwrap();
    // Pinned target + script, but no oracle → grading would be undefined.
    let manifest = write_manifest(
        dir.path(),
        "no_oracle.json",
        r#"{"snapshot":"s","targets":{"pinned":[{"id":"t","project":"p","commit":"c","harness":"h","file":"f","line":1,"family":"frontier"}]},"script":{"t":{"input":"x","confidence":0.9}}}"#,
    );
    let err =
        dispatch_with_home(dir.path(), args(&["run", "m", "--targets", &manifest])).unwrap_err();
    assert!(err.to_string().contains("oracle"));
}

#[test]
fn run_rejects_manifest_without_script() {
    let dir = tempfile::tempdir().unwrap();
    // Pinned target + oracle, but no candidate script → all-N hollow run.
    let manifest = write_manifest(
        dir.path(),
        "no_script.json",
        r#"{"snapshot":"s","targets":{"pinned":[{"id":"t","project":"p","commit":"c","harness":"h","file":"f","line":1,"family":"frontier"}]},"oracle":{"t":"x"}}"#,
    );
    let err =
        dispatch_with_home(dir.path(), args(&["run", "m", "--targets", &manifest])).unwrap_err();
    assert!(err.to_string().contains("script"));
}

#[test]
fn run_rejects_manifest_with_disjoint_script_keys() {
    let dir = tempfile::tempdir().unwrap();
    // Script exists but is keyed to a non-pinned id → every pinned target would
    // yield NoSubmission (a hollow all-N run). Must be rejected.
    let manifest = write_manifest(
        dir.path(),
        "disjoint.json",
        r#"{"snapshot":"s","targets":{"pinned":[{"id":"t1","project":"p","commit":"c","harness":"h","file":"f","line":1,"family":"frontier"}]},"oracle":{"t1":"x"},"script":{"not-t1":{"input":"y","confidence":0.9}}}"#,
    );
    let err =
        dispatch_with_home(dir.path(), args(&["run", "m", "--targets", &manifest])).unwrap_err();
    assert!(err.to_string().contains("pinned target"));
}

#[test]
fn contract_command_runs_with_defaults_and_flags() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // Default snapshot.
    dispatch_with_home(home, args(&["contract"])).unwrap();
    // Explicit snapshot + repeated split/project (comma-separated) + source.
    dispatch_with_home(
        home,
        args(&[
            "contract",
            "--dataset",
            "COIN-Bench/coin",
            "--revision",
            "v2026-07",
            "--split",
            "codeql_only,gcs_reachable",
            "--project",
            "cups,libraw",
            "--source",
            "image",
        ]),
    )
    .unwrap();
}

#[test]
fn contract_rejects_unknown_source() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_with_home(dir.path(), args(&["contract", "--source", "magic"])).unwrap_err();
    assert!(err.to_string().contains("unknown --source"));
}

/// The Phase-5 loop fixture (decoder+crypto generalise; generic overfits).
const LOOP_SNAPSHOT: &str = include_str!("fixtures/improve_loop_snapshot.json");

/// The single run-id under a profile's runs directory.
fn only_run_id(home: &std::path::Path, profile: &str) -> String {
    let runs = runs_dir(home, profile);
    let mut ids: Vec<String> = std::fs::read_dir(&runs)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .map(|e| e.path().file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(ids.len(), 1, "expected exactly one run under {profile}");
    ids.pop().unwrap()
}

#[test]
fn improve_holdout_fresh_runs_full_cycle_via_cli() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let manifest = write_manifest(home, "loop.json", LOOP_SNAPSHOT);

    // A baseline run persists the offline scaffold (oracle + script) the loop needs.
    dispatch_with_home(
        home,
        args(&["run", "m", "--targets", &manifest, "--profile", "loop"]),
    )
    .unwrap();
    let run_id = only_run_id(home, "loop");

    // The live self-improvement cycle runs end-to-end and banks durable tactics.
    dispatch_with_home(
        home,
        args(&[
            "improve",
            &run_id,
            "--profile",
            "loop",
            "--holdout",
            "fresh",
        ]),
    )
    .unwrap();

    let tactics = super::improve_loop::load_tactic_memory(home, "loop").unwrap();
    assert_eq!(tactics.tactics.len(), 2, "decoder + crypto tactics banked");
}

#[test]
fn improve_holdout_rejects_unknown_mode() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let manifest = write_manifest(home, "loop.json", LOOP_SNAPSHOT);
    dispatch_with_home(
        home,
        args(&["run", "m", "--targets", &manifest, "--profile", "loop"]),
    )
    .unwrap();
    let run_id = only_run_id(home, "loop");
    let err = dispatch_with_home(
        home,
        args(&[
            "improve",
            &run_id,
            "--profile",
            "loop",
            "--holdout",
            "stale",
        ]),
    )
    .unwrap_err();
    assert!(err.to_string().contains("only supports 'fresh'"));
}
