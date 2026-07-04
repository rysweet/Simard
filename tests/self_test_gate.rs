//! End-to-end regression for the self-test / gym run-suite false-green
//! (rysweet/Simard#2548).
//!
//! Before the fix, `gym run-suite starter` printed `Suite passed: false` but
//! always exited 0, so `self-test` reported `SELF-TEST PASSED` even when the
//! starter suite failed and the `self-update` relaunch gate was a no-op.
//!
//! These tests drive the real `simard` binary and pin the honest contract:
//!
//! * a healthy binary's `self-test` is *genuinely* green — deterministic pass
//!   with a zero exit and a `SELF-TEST PASSED` line;
//! * `gym run-suite starter` reflects the suite's real pass/fail in its exit
//!   code (the wiring that makes `self-test` trustworthy);
//! * an unknown suite still exits non-zero.
//!
//! The failing-suite → non-zero-exit → `self-test` FAILED → relaunch-refused
//! half of the chain is pinned deterministically by unit tests in
//! `src/operator_commands_gym/commands.rs` (`evaluate_suite_result`) and
//! `src/cmd_self_update/update.rs`
//! (`self_update_relaunch_gate_rejects_failing_self_test`).

use assert_cmd::Command;

/// A fresh, isolated working directory so the suite's `target/simard-gym`
/// artifacts do not collide with other tests running concurrently.
fn isolated_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-2548-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create isolated dir");
    dir
}

fn simard(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("simard").expect("simard binary must be buildable");
    // Never touch the network for the update check during the test.
    cmd.env("SIMARD_NO_UPDATE_CHECK", "1");
    cmd.current_dir(dir);
    cmd
}

#[test]
fn self_test_on_healthy_binary_is_genuinely_green() {
    let dir = isolated_dir("self-test");
    let assert = simard(&dir).arg("self-test").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    assert!(
        stdout.contains("Suite passed: true"),
        "starter gate must pass on a healthy binary:\n{stdout}"
    );
    assert!(
        stdout.contains("SELF-TEST PASSED"),
        "self-test must report PASSED on a green gate:\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_suite_starter_passes_and_exits_zero() {
    let dir = isolated_dir("run-suite");
    let assert = simard(&dir)
        .args(["gym", "run-suite", "starter"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    assert!(
        stdout.contains("Suite: starter") && stdout.contains("Suite passed: true"),
        "starter suite must be deterministically green:\n{stdout}"
    );
    // The gate runs only the deterministic session-quality scenarios; the
    // LLM-content-check benchmarks are excluded so they cannot false-green it.
    assert!(
        stdout.contains("composite-session-review: passed")
            && stdout.contains("interactive-terminal-driving: passed")
            && stdout.contains("session-quality-memory-export: passed"),
        "gate must run the deterministic session-quality scenarios:\n{stdout}"
    );
    assert!(
        !stdout.contains("repo-exploration-local"),
        "LLM-content-check scenarios must not gate self-test:\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_suite_unknown_suite_exits_non_zero() {
    let dir = isolated_dir("bogus");
    simard(&dir)
        .args(["gym", "run-suite", "does-not-exist"])
        .assert()
        .failure();
    let _ = std::fs::remove_dir_all(&dir);
}
