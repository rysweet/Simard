//! Integration regression tests for the Copilot engineer subprocess
//! permission contract (issue #1717).
//!
//! ## What this file pins
//!
//! Every dispatched Copilot engineer subprocess MUST receive both
//! `--allow-all-tools` and `--allow-all-paths` (in that order, before `-p`)
//! and MUST be spawned with `COPILOT_ALLOW_ALL=1` in its environment. Without
//! these, every engineer plan ends with a permission-denied table because the
//! Copilot CLI's tool allow-list defaults to interactive prompting and there
//! is no TTY to confirm.
//!
//! ## How the assertions are observed
//!
//! These tests do NOT invoke the real Copilot CLI. They spawn the Simard
//! engineer-loop helper (`run_engineer_subprocess`) against a *stub*
//! `amplihack` shim — a small bash script in a `TempDir` — that:
//!
//!   1. Records every argv element it received, one per line, to
//!      `observations.log`.
//!   2. Records the value of `COPILOT_ALLOW_ALL` (and a small whitelist of
//!      sibling env vars) to the same log.
//!   3. Exits 0 so the helper returns `Ok(_)` and we can assert against
//!      the captured trace.
//!
//! The shim is selected via the `SIMARD_AMPLIHACK_BIN` env var (already used
//! by other tests in this repo to redirect the binary lookup), which avoids
//! mutating `PATH` and the cross-test races that come with it.
//!
//! ## Symptom this test prevents from regressing
//!
//! Snapshot referenced in PR #1717:
//! `~/.simard/wip-snapshots/amplihack-hygiene-plan-20260512T231555Z.md` —
//! a thoughtful engineer plan whose final lines were a permission-denied
//! table for `echo > file`, `tee`, `gh issue create`, `gh pr create`,
//! `git checkout -b`, `git commit`, `git push`, and `amplihack recipe run`.
//! All ten failed because `--allow-all-tools` was not set.

use std::path::{Path, PathBuf};

use serial_test::serial;
use simard::engineer_loop::{AgentKind, engineer_argv, run_engineer_subprocess};

/// RAII guard for `SIMARD_AMPLIHACK_BIN`. Restores the prior value (or
/// removes the variable) on `Drop` so concurrent / subsequent tests are not
/// affected by an uncaught panic mid-test.
struct AmplihackBinEnv {
    prior: Option<String>,
}

impl AmplihackBinEnv {
    fn set(value: &Path) -> Self {
        let prior = std::env::var("SIMARD_AMPLIHACK_BIN").ok();
        // SAFETY: tests in this file are `#[serial(simard_amplihack_bin_env)]`
        // so no two of them mutate this var concurrently.
        unsafe {
            std::env::set_var("SIMARD_AMPLIHACK_BIN", value);
        }
        Self { prior }
    }
}

impl Drop for AmplihackBinEnv {
    fn drop(&mut self) {
        // SAFETY: see `set` above.
        unsafe {
            match self.prior.take() {
                Some(v) => std::env::set_var("SIMARD_AMPLIHACK_BIN", v),
                None => std::env::remove_var("SIMARD_AMPLIHACK_BIN"),
            }
        }
    }
}

/// Write a bash shim at `dir/amplihack` that captures argv + a slice of env
/// to `dir/observations.log` and exits 0. Returns the path to the shim.
///
/// The shim format is intentionally simple so the assertions below can use
/// plain substring matching rather than a parser.
fn write_amplihack_observation_shim(dir: &Path) -> PathBuf {
    let shim_path = dir.join("amplihack");
    let log_path = dir.join("observations.log");
    let log_path_str = log_path.to_string_lossy();
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
LOG="{log}"
{{
    echo "BEGIN_ARGV"
    for a in "$@"; do
        echo "ARG: $a"
    done
    echo "END_ARGV"
    echo "ENV: COPILOT_ALLOW_ALL=${{COPILOT_ALLOW_ALL:-<unset>}}"
    # Whitelisted sibling env keys so a future regression that drops the
    # parent-env passthrough is also caught.
    echo "ENV: PATH_SET=${{PATH:+yes}}"
    echo "DONE"
}} >> "$LOG"
exit 0
"#,
        log = log_path_str
    );
    std::fs::write(&shim_path, script).expect("write amplihack shim");

    // chmod +x
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&shim_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&shim_path, perms).unwrap();
    }

    shim_path
}

fn read_observations(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("observations.log")).expect("observations.log written")
}

// ---------------------------------------------------------------------------
// Test 1 — argv contract (pure, no subprocess)
//
// Mirrors the unit tests in `agent_spawn.rs::tests` but exercises the
// re-exported test-visible API to verify the surface stays callable from
// `tests/`. This is the cheapest test in the file; if this fails, every
// other test in this file is also expected to fail.
// ---------------------------------------------------------------------------

#[test]
fn copilot_argv_contract_includes_both_permission_flags_in_order() {
    let argv = engineer_argv(AgentKind::Copilot, "objective", 7);

    let tools_pos = argv
        .iter()
        .position(|a| a == "--allow-all-tools")
        .expect("--allow-all-tools must be present (issue #1717)");
    let paths_pos = argv
        .iter()
        .position(|a| a == "--allow-all-paths")
        .expect("--allow-all-paths must be present");
    let p_pos = argv
        .iter()
        .position(|a| a == "-p")
        .expect("-p must be present");

    assert!(
        tools_pos < paths_pos,
        "--allow-all-tools must precede --allow-all-paths: {argv:?}"
    );
    assert!(
        paths_pos < p_pos,
        "permission flags must precede -p: {argv:?}"
    );

    // Forbidden tokens — Copilot CLI does not accept these and will reject
    // the whole invocation if they leak in from a future refactor.
    assert!(
        !argv.iter().any(|a| a == "--"),
        "Copilot argv must not include the `--` separator: {argv:?}"
    );
    assert!(
        !argv.iter().any(|a| a == "--auto"),
        "--auto is RustyClawd-only; must not appear for Copilot: {argv:?}"
    );
    assert!(
        !argv.iter().any(|a| a == "--max-turns"),
        "--max-turns is RustyClawd-only; must not appear for Copilot: {argv:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — observed argv reaches the spawned subprocess
//
// Spawn the helper with the env-redirected `amplihack` pointing at the
// observation shim. Read back the log and assert both permission flags
// landed in the spawned process's argv.
// ---------------------------------------------------------------------------

#[test]
#[serial(simard_amplihack_bin_env)]
fn spawned_copilot_subprocess_receives_allow_all_tools_in_argv() {
    let dir = tempfile::tempdir().expect("tempdir");
    let shim = write_amplihack_observation_shim(dir.path());
    let _guard = AmplihackBinEnv::set(&shim);

    let result = run_engineer_subprocess("engineer prompt body", dir.path(), AgentKind::Copilot);
    assert!(
        result.is_ok(),
        "stub shim should exit 0 and helper should return Ok; got: {result:?}"
    );

    let log = read_observations(dir.path());
    assert!(
        log.contains("ARG: --allow-all-tools"),
        "spawned subprocess argv must include --allow-all-tools (issue #1717); \
         observations:\n{log}"
    );
    assert!(
        log.contains("ARG: --allow-all-paths"),
        "spawned subprocess argv must include --allow-all-paths; \
         observations:\n{log}"
    );
    assert!(
        log.contains("ARG: copilot"),
        "spawned subprocess argv must start with the `copilot` subcommand; \
         observations:\n{log}"
    );
    assert!(
        log.contains("ARG: -p"),
        "spawned subprocess argv must include -p; observations:\n{log}"
    );
    assert!(
        log.contains("ARG: engineer prompt body"),
        "spawned subprocess argv must include the literal prompt; \
         observations:\n{log}"
    );

    // Anti-regression: --allow-all-tools must come BEFORE -p in the captured
    // log line ordering. We check this by looking up byte positions.
    let tools_idx = log
        .find("ARG: --allow-all-tools")
        .expect("flag presence already asserted");
    let p_idx = log.find("ARG: -p").expect("flag presence already asserted");
    assert!(
        tools_idx < p_idx,
        "--allow-all-tools must precede -p in the spawned argv ordering; \
         observations:\n{log}"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — observed env contains COPILOT_ALLOW_ALL=1
//
// The env var is the belt-and-suspenders fallback documented in
// docs/reference/engineer-copilot-permissions.md. If the upstream Copilot
// CLI ever renames `--allow-all-tools`, the env var keeps the contract
// degrading gracefully. This test pins that the env var is actually being
// passed through `Command::env`, not just documented.
// ---------------------------------------------------------------------------

#[test]
#[serial(simard_amplihack_bin_env)]
fn spawned_copilot_subprocess_inherits_copilot_allow_all_env() {
    let dir = tempfile::tempdir().expect("tempdir");
    let shim = write_amplihack_observation_shim(dir.path());
    let _guard = AmplihackBinEnv::set(&shim);

    let result = run_engineer_subprocess("another prompt", dir.path(), AgentKind::Copilot);
    assert!(result.is_ok(), "stub shim should succeed; got: {result:?}");

    let log = read_observations(dir.path());
    assert!(
        log.contains("ENV: COPILOT_ALLOW_ALL=1"),
        "spawned subprocess must have COPILOT_ALLOW_ALL=1 in its environment \
         (belt-and-suspenders fallback for upstream flag renames; \
         issue #1717); observations:\n{log}"
    );
    // Sanity check: parent PATH still inherited.
    assert!(
        log.contains("ENV: PATH_SET=yes"),
        "parent PATH must still be inherited by the spawned subprocess; \
         observations:\n{log}"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — write/commit/PR symptom regression
//
// Exercises the original symptom: an engineer with a prompt that asks it to
// (a) write a file, (b) commit, (c) `gh pr create`. Because the stub shim
// is what the helper actually invokes (not the real Copilot CLI), this test
// CANNOT prove the engineer would succeed end-to-end — the real Copilot CLI
// is what would honor `--allow-all-tools`. What this test CAN prove is that
// the subprocess receives the right permission grant *and* that callers
// observe a successful completion (`Ok(summary)`), which is the necessary
// precondition for the real engineer to do useful work.
//
// This is the regression test the PR description is allowed to cite as the
// "engineer can write/commit/PR" coverage.
// ---------------------------------------------------------------------------

#[test]
#[serial(simard_amplihack_bin_env)]
fn engineer_dispatch_with_write_commit_pr_prompt_returns_ok_with_full_grant() {
    let dir = tempfile::tempdir().expect("tempdir");
    let shim = write_amplihack_observation_shim(dir.path());
    let _guard = AmplihackBinEnv::set(&shim);

    // A prompt that mentions write/commit/PR so the test name lines up with
    // the symptom it regresses; the stub does not actually evaluate the
    // prompt content.
    let prompt = "Write CHANGELOG.md, run `git commit -am wip`, run \
                  `gh pr create --title fix --body fix`, then summarize.";
    let result = run_engineer_subprocess(prompt, dir.path(), AgentKind::Copilot);

    assert!(
        result.is_ok(),
        "engineer dispatch must return Ok when the subprocess exits 0 — \
         the original bug surfaced as a non-zero exit + permission table; \
         got: {result:?}"
    );

    let log = read_observations(dir.path());

    // Verify the contract that allows the real Copilot to actually do the
    // write/commit/PR work was passed through.
    assert!(
        log.contains("ARG: --allow-all-tools"),
        "write/commit/PR engineer dispatch missing --allow-all-tools; \
         observations:\n{log}"
    );
    assert!(
        log.contains("ENV: COPILOT_ALLOW_ALL=1"),
        "write/commit/PR engineer dispatch missing COPILOT_ALLOW_ALL=1; \
         observations:\n{log}"
    );
    assert!(
        log.contains(prompt),
        "the literal engineer prompt must reach the subprocess argv; \
         observations:\n{log}"
    );
}
