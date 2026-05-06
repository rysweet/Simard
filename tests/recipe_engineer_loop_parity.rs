//! Interface tests: the recipe-driven engineer-loop helper bin (`simard-engineer-step`)
//! exposes the correct subcommand surface after the agent-spawn refactor (#1536).
//!
//! The old plan-parse-execute phases (select, execute, verify) were removed;
//! they are replaced by a single `agent-spawn` subcommand that delegates to a
//! subordinate Copilot agent session. Tests that require a live LLM are
//! marked `#[ignore]`.
//!
//! Phases and why each is (or isn't) tested here:
//!   - inspect:     requires a live git workspace; covered by smoke test
//!   - agent-spawn: requires a live LLM; covered by integration tests
//!   - review:      agentic step; covered by recipe-runner integration tests
//!   - persist:     filesystem writes; covered by tests_review_persist suite
//!
//! This file validates the binary's subcommand routing and error-handling
//! for malformed inputs — tests that do NOT require a live LLM or git workspace.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use simard::engineer_loop::RepoInspection;

fn helper_bin() -> String {
    let target = std::env::var("CARGO_TARGET_DIR")
        .unwrap_or_else(|_| format!("{}/target", env!("CARGO_MANIFEST_DIR")));
    format!("{target}/debug/simard-engineer-step")
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

/// Removed subcommands (select, execute, verify) must exit 2 with a useful error.
#[test]
fn removed_subcommands_exit_nonzero() {
    for subcmd in ["select", "execute", "verify"] {
        let out = Command::new(helper_bin())
            .args([subcmd, "--objective", "anything"])
            .stdin(Stdio::null())
            .output()
            .expect("spawn simard-engineer-step");
        assert!(
            !out.status.success(),
            "removed '{subcmd}' subcommand must not succeed"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("unknown subcommand"),
            "stderr should mention unknown subcommand for '{subcmd}', got: {stderr}"
        );
    }
}

/// `agent-spawn` with missing required flags must exit 2.
#[test]
fn agent_spawn_missing_flags_exits_nonzero() {
    let out = Command::new(helper_bin())
        .args(["agent-spawn"])
        .stdin(Stdio::null())
        .output()
        .expect("spawn simard-engineer-step");
    assert!(
        !out.status.success(),
        "agent-spawn with no flags must exit non-zero"
    );
}

/// `agent-spawn` with malformed inspection JSON must exit 2 and mention a parse error.
#[test]
fn agent_spawn_bad_inspection_json_exits_nonzero() {
    let out = Command::new(helper_bin())
        .args([
            "agent-spawn",
            "--inspection-json",
            "not-valid-json",
            "--objective",
            "do something",
            "--workspace",
            "/tmp",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("spawn simard-engineer-step");
    assert!(
        !out.status.success(),
        "agent-spawn with bad JSON must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("parse inspection-json") || stderr.contains("expected"),
        "stderr should mention parse failure, got: {stderr}"
    );
}

/// `agent-spawn` with a valid inspection JSON requires a live LLM to succeed.
#[test]
#[ignore = "requires live LLM provider (SIMARD_LLM_PROVIDER)"]
fn agent_spawn_live_lm_required() {
    let inspection = fixture_inspection();
    let inspection_json = serde_json::to_string(&inspection).unwrap();
    let out = Command::new(helper_bin())
        .args([
            "agent-spawn",
            "--inspection-json",
            &inspection_json,
            "--objective",
            "look around and report back",
            "--workspace",
            "/tmp/parity-workspace",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("spawn simard-engineer-step");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "agent-spawn should succeed with live LLM; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("completed"),
        "stdout should contain status=completed, got: {stdout}"
    );
}
