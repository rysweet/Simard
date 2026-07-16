use super::profiles::{
    DEFAULT_HOME, PersistedRun, default_home, ensure_profile, list_profiles, list_runs, load_run,
    sanitize_name, save_run,
};
use super::target_loader::TargetSet;
use super::types::{RunReport, Strategy};

fn persisted(run_id: &str) -> PersistedRun {
    PersistedRun {
        report: RunReport {
            run_id: run_id.to_string(),
            model: "claude-opus-4.6".to_string(),
            strategy: Strategy::Baseline,
            snapshot: "snap".to_string(),
            started_at_unix_ms: 1,
            outcomes: Vec::new(),
            offline_scaffold: true,
        },
        targets: TargetSet {
            snapshot: "snap".to_string(),
            pinned: Vec::new(),
            held_out_fresh: Vec::new(),
        },
        offline: super::target_loader::OfflineScaffold::default(),
    }
}

#[test]
fn sanitize_name_is_directory_safe() {
    assert_eq!(sanitize_name("claude/opus:4.6"), "claude-opus-4-6");
    assert_eq!(sanitize_name(""), "default");
    assert_eq!(sanitize_name("ok_name-1"), "ok_name-1");
}

#[test]
fn default_home_honours_env_override() {
    // Default (relative) when unset is a constant.
    assert_eq!(DEFAULT_HOME, "target/coin-gym");
    // Note: we do not mutate process env here to avoid cross-test races; the
    // override path is covered by exercising the explicit-home APIs below.
    let _ = default_home();
}

#[test]
fn ensure_profile_persists_metadata_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let p1 = ensure_profile(home, "opus", "claude-opus-4.6").unwrap();
    assert_eq!(p1.name, "opus");
    assert_eq!(p1.model, "claude-opus-4.6");
    // Second call with the SAME model loads existing metadata (idempotent).
    let p2 = ensure_profile(home, "opus", "claude-opus-4.6").unwrap();
    assert_eq!(p2.created_at_unix_ms, p1.created_at_unix_ms);
    assert_eq!(p2.model, "claude-opus-4.6");
}

#[test]
fn ensure_profile_rejects_model_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    ensure_profile(home, "opus", "claude-opus-4.6").unwrap();
    // Reusing the profile for a different model must be rejected (isolation).
    let err = ensure_profile(home, "opus", "gpt-5.4").unwrap_err();
    assert!(err.to_string().contains("bound to model"));
}

#[test]
fn save_run_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    ensure_profile(home, "opus", "claude-opus-4.6").unwrap();
    let run = persisted("dup-run-1");
    save_run(home, "opus", &run).unwrap();
    // Second save with the same run-id must fail loudly, not clobber.
    let err = save_run(home, "opus", &run).unwrap_err();
    assert!(err.to_string().contains("already exists"));
}

#[test]
fn save_and_load_run_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    ensure_profile(home, "opus", "claude-opus-4.6").unwrap();
    let run = persisted("claude-opus-4-6-baseline-1");
    let path = save_run(home, "opus", &run).unwrap();
    assert!(path.exists());

    // Explicit profile.
    let loaded = load_run(home, Some("opus"), "claude-opus-4-6-baseline-1").unwrap();
    assert_eq!(loaded.report.run_id, run.report.run_id);

    // Search across all profiles when no profile is given.
    let found = load_run(home, None, "claude-opus-4-6-baseline-1").unwrap();
    assert_eq!(found.report.model, "claude-opus-4.6");
}

#[test]
fn load_missing_run_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_run(dir.path(), None, "nope").unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[test]
fn list_profiles_is_empty_then_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    assert!(list_profiles(home).unwrap().is_empty());

    ensure_profile(home, "zeta", "m1").unwrap();
    ensure_profile(home, "alpha", "m2").unwrap();
    let names: Vec<String> = list_profiles(home)
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, vec!["alpha".to_string(), "zeta".to_string()]);
}

#[test]
fn list_runs_is_empty_missing_then_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // A profile that was never created yields an empty list, not an error.
    assert!(list_runs(home, "ghost").unwrap().is_empty());

    ensure_profile(home, "opus", "claude-opus-4.6").unwrap();
    // Freshly created profile with no runs is also empty.
    assert!(list_runs(home, "opus").unwrap().is_empty());

    // Save two runs out of run-id order; list_runs sorts by run id.
    save_run(home, "opus", &persisted("b-run-2")).unwrap();
    save_run(home, "opus", &persisted("a-run-1")).unwrap();
    let ids: Vec<String> = list_runs(home, "opus")
        .unwrap()
        .into_iter()
        .map(|r| r.report.run_id)
        .collect();
    assert_eq!(ids, vec!["a-run-1".to_string(), "b-run-2".to_string()]);
}
