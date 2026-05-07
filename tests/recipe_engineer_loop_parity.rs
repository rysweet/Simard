//! Parity test: the recipe-driven engineer-loop helper bin produces results
//! equivalent to direct Rust function calls for the deterministic phases
//! (select, verify). This locks the architectural pivot into CI: the recipe
//! path must remain equivalent to the Rust path for these phases when the
//! type surface evolves in the future.
//!
//! Phases not covered here (and why):
//!   - inspect: requires a live git workspace; covered by smoke test
//!   - execute: actually runs git/shell commands; covered by smoke test
//!   - persist: writes to the filesystem; covered by existing
//!     tests_review_persist suite
//!   - review: agentic step; covered by recipe-runner integration tests

use std::path::PathBuf;
use std::process::{Command, Stdio};

use simard::engineer_loop::{
    EngineerActionKind, ExecutedEngineerAction, RepoInspection, SelectedEngineerAction,
    VerificationReport, select_engineer_action, verify_engineer_action,
};

fn helper_bin() -> String {
    let target = std::env::var("CARGO_TARGET_DIR")
        .unwrap_or_else(|_| format!("{}/target", env!("CARGO_MANIFEST_DIR")));
    format!("{target}/debug/simard-engineer-step")
}

fn run_helper(args: &[&str]) -> String {
    let out = Command::new(helper_bin())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn simard-engineer-step");
    if !out.status.success() {
        panic!(
            "helper {:?} failed: status={} stderr={}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn fixture_inspection() -> RepoInspection {
    RepoInspection {
        workspace_root: PathBuf::from("/tmp/parity-workspace"),
        repo_root: PathBuf::from("/tmp/parity-workspace"),
        branch: "main".to_string(),
        head: "0000000000000000000000000000000000000000".to_string(),
        worktree_dirty: false,
        changed_files: Vec::new(),
        active_goals: Vec::new(),
        carried_meeting_decisions: Vec::new(),
        architecture_gap_summary: String::new(),
    }
}

#[test]
#[ignore = "requires live LLM provider (SIMARD_LLM_PROVIDER); planner has no fallback"]
fn parity_select_create_file_objective() {
    let inspection = fixture_inspection();
    let inspection_json = serde_json::to_string(&inspection).unwrap();
    let objective = "create a new file docs/notes.md";

    let rust = select_engineer_action(&inspection, objective).expect("rust select ok");
    let recipe_stdout = run_helper(&[
        "select",
        "--inspection-json",
        &inspection_json,
        "--objective",
        objective,
    ]);
    let recipe: SelectedEngineerAction =
        serde_json::from_str(&recipe_stdout).expect("parse selected");

    assert_eq!(rust, recipe, "select mismatch for create-file objective");
}

#[test]
#[ignore = "requires live LLM provider (SIMARD_LLM_PROVIDER); planner has no fallback"]
fn parity_select_cargo_test_objective_both_paths_error() {
    // Without a Cargo.toml in the workspace, both paths should reject this
    // objective with the same UnsupportedEngineerAction error. The recipe
    // path must fail-fast just like the Rust path.
    let inspection = fixture_inspection();
    let inspection_json = serde_json::to_string(&inspection).unwrap();
    let objective = "run the test suite";

    let rust_err = select_engineer_action(&inspection, objective)
        .expect_err("rust select should error without Cargo.toml");
    assert!(
        rust_err.to_string().contains("local-first action policy")
            || rust_err.to_string().contains("UnsupportedEngineerAction"),
        "unexpected rust error: {rust_err}"
    );

    let out = std::process::Command::new(helper_bin())
        .args([
            "select",
            "--inspection-json",
            &inspection_json,
            "--objective",
            objective,
        ])
        .output()
        .expect("spawn helper");
    assert!(!out.status.success(), "recipe select should fail-fast too");
}

#[test]
#[ignore = "requires live LLM provider (SIMARD_LLM_PROVIDER); planner has no fallback"]
fn parity_select_read_only_scan_default() {
    let inspection = fixture_inspection();
    let inspection_json = serde_json::to_string(&inspection).unwrap();
    let objective = "look around and report status";

    let rust = select_engineer_action(&inspection, objective).expect("rust select ok");
    let recipe_stdout = run_helper(&[
        "select",
        "--inspection-json",
        &inspection_json,
        "--objective",
        objective,
    ]);
    let recipe: SelectedEngineerAction =
        serde_json::from_str(&recipe_stdout).expect("parse selected");

    assert_eq!(rust, recipe, "select mismatch for read-only-scan objective");
}

#[test]
fn parity_verify_failed_action_returns_error() {
    // Both paths should bubble an error when the action exit code is non-zero.
    let tmp = tempfile::tempdir().expect("tempdir");
    let inspection = RepoInspection {
        workspace_root: tmp.path().to_path_buf(),
        repo_root: tmp.path().to_path_buf(),
        ..fixture_inspection()
    };
    let inspection_json = serde_json::to_string(&inspection).unwrap();

    let selected = SelectedEngineerAction {
        label: "test".to_string(),
        rationale: "test".to_string(),
        argv: vec!["echo".to_string(), "hi".to_string()],
        plan_summary: "p".to_string(),
        verification_steps: vec!["s".to_string()],
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::ReadOnlyScan,
    };
    let action = ExecutedEngineerAction {
        selected,
        exit_code: 1,
        stdout: String::new(),
        stderr: "boom".to_string(),
        changed_files: Vec::new(),
    };
    let action_json = serde_json::to_string(&action).unwrap();

    let rust = verify_engineer_action(&inspection, &action, tmp.path());
    assert!(rust.is_err(), "rust verify should error on non-zero exit");

    // Recipe path: helper bin should also exit non-zero
    let out = std::process::Command::new(helper_bin())
        .args([
            "verify",
            "--inspection-json",
            &inspection_json,
            "--action-json",
            &action_json,
            "--state-root",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .expect("spawn helper");
    assert!(
        !out.status.success(),
        "recipe verify should fail on non-zero exit"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("verify_engineer_action failed"),
        "stderr should mention verify failure: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn parity_verify_clean_read_only_scan_succeeds() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // verify_engineer_action checks the git worktree state — initialize one
    let init = std::process::Command::new("git")
        .args(["init", "--quiet", tmp.path().to_str().unwrap()])
        .status()
        .expect("git init");
    assert!(init.success(), "git init failed");
    // Need an initial commit for HEAD to resolve
    for cmd in [
        vec![
            "-C",
            tmp.path().to_str().unwrap(),
            "config",
            "user.email",
            "test@test",
        ],
        vec![
            "-C",
            tmp.path().to_str().unwrap(),
            "config",
            "user.name",
            "test",
        ],
        vec![
            "-C",
            tmp.path().to_str().unwrap(),
            "commit",
            "--allow-empty",
            "-m",
            "init",
            "--quiet",
        ],
    ] {
        let s = std::process::Command::new("git")
            .args(&cmd)
            .status()
            .expect("git");
        assert!(s.success(), "git {cmd:?} failed");
    }

    // Determine the actual branch + HEAD so inspection matches
    let branch_out = std::process::Command::new("git")
        .args([
            "-C",
            tmp.path().to_str().unwrap(),
            "rev-parse",
            "--abbrev-ref",
            "HEAD",
        ])
        .output()
        .expect("git branch");
    let branch = String::from_utf8(branch_out.stdout)
        .unwrap()
        .trim()
        .to_string();
    let head_out = std::process::Command::new("git")
        .args(["-C", tmp.path().to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse");
    let head = String::from_utf8(head_out.stdout)
        .unwrap()
        .trim()
        .to_string();

    let inspection = RepoInspection {
        workspace_root: tmp.path().to_path_buf(),
        repo_root: tmp.path().to_path_buf(),
        branch,
        head,
        ..fixture_inspection()
    };
    let inspection_json = serde_json::to_string(&inspection).unwrap();

    let selected = SelectedEngineerAction {
        label: "git-tracked-file-scan".to_string(),
        rationale: "look".to_string(),
        argv: vec!["git".to_string(), "ls-files".to_string()],
        plan_summary: "p".to_string(),
        verification_steps: vec!["s".to_string()],
        expected_changed_files: Vec::new(),
        kind: EngineerActionKind::ReadOnlyScan,
    };
    let action = ExecutedEngineerAction {
        selected,
        exit_code: 0,
        stdout: "Cargo.toml\nsrc/lib.rs\n".to_string(),
        stderr: String::new(),
        changed_files: Vec::new(),
    };
    let action_json = serde_json::to_string(&action).unwrap();

    let rust = verify_engineer_action(&inspection, &action, tmp.path()).expect("rust verify ok");

    let recipe_stdout = run_helper(&[
        "verify",
        "--inspection-json",
        &inspection_json,
        "--action-json",
        &action_json,
        "--state-root",
        tmp.path().to_str().unwrap(),
    ]);
    let recipe: VerificationReport =
        serde_json::from_str(&recipe_stdout).expect("parse verification");

    assert_eq!(rust, recipe, "verify mismatch for clean read-only scan");
}
