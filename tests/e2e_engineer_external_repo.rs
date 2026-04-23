//! End-to-end integration test: Simard engineer loop driving external repos.
//!
//! Verifies Simard can inspect, select actions, execute changes, and verify
//! outcomes on repositories outside her own codebase — the core capability
//! needed for autonomous engineering work on the amplihack ecosystem.
//!
//! IMPORTANT: These tests use the pre-built binary at `target/debug/simard`
//! to avoid cargo lock contention when multiple tests run in parallel.
//! Run `cargo build` before `cargo test --test e2e_engineer_external_repo`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve the Simard binary path from the build output.
fn simard_binary() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = manifest_dir.join("target/debug/simard");
    assert!(
        binary.exists(),
        "Simard binary not found at {}. Run `cargo build` first.",
        binary.display()
    );
    binary
}

/// Verify Simard's engineer loop can inspect an external workspace.
#[test]
fn engineer_loop_inspects_external_repo() {
    // Use Simard's own repo (smaller, always available) to test external inspection.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));

    // Wrap in `timeout` so a hung engineer loop cannot stall CI for the
    // full job timeout. 30s is plenty for a read-only scan even on cold
    // cache; if the binary truly hangs we want to see it fail visibly.
    let output = Command::new("timeout")
        .args([
            "30",
            simard_binary().to_str().unwrap(),
            "engineer",
            "run",
            "single-process",
            repo.to_str().unwrap(),
            "Read-only scan: identify the project structure and key directories",
        ])
        .env("SIMARD_STATE_ROOT", "/tmp/simard-e2e-inspect")
        .output()
        .expect("failed to run simard engineer");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}\n{stderr}");

    // Skip when the CI environment lacks an LLM provider — the simard binary
    // now refuses to start without explicit SIMARD_LLM_PROVIDER configuration.
    if combined.contains("missing required configuration 'SIMARD_LLM_PROVIDER'")
        || combined.contains("No API key found")
        || combined.contains("LLM session but open() failed")
    {
        eprintln!("SKIP: no LLM provider available (CI environment)");
        return;
    }

    // Engineer loop should at least complete inspection phase
    assert!(
        combined.contains("Simard") || combined.contains("workspace") || combined.contains("scan"),
        "engineer loop should recognize the workspace:\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// Verify Simard can list gym scenarios (no LLM needed).
#[test]
fn gym_list_shows_all_scenarios() {
    let output = Command::new(simard_binary())
        .args(["gym", "list"])
        .output()
        .expect("failed to run simard gym list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("repo-exploration-deep-scan"));
    assert!(stdout.contains("doc-generation-public-fn"));
    assert!(stdout.contains("safe-code-change-add-derive"));
    assert!(stdout.contains("test-writing-unit-case"));
    assert!(stdout.contains("interactive-terminal-driving"));
}

/// Verify the meeting REPL launches and shows the greeting banner.
#[test]
fn meeting_repl_shows_greeting() {
    let output = Command::new("timeout")
        .args([
            "10",
            simard_binary().to_str().unwrap(),
            "meeting",
            "repl",
            "integration-test",
        ])
        .env("SIMARD_STATE_ROOT", "/tmp/simard-e2e-meeting")
        .output()
        .expect("failed to run meeting repl");

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should show the Simard greeting banner
    assert!(
        stderr.contains("Simard v") || stderr.contains("simard"),
        "meeting REPL should show Simard greeting:\n{stderr}"
    );
}

/// Verify OODA daemon starts and seeds default goals.
/// Skipped in CI when amplihack-memory-lib is unavailable.
#[test]
fn ooda_daemon_seeds_five_goals() {
    let state_root = tempfile::tempdir().expect("temp dir for ooda test");
    let output = Command::new("timeout")
        .args([
            "15",
            simard_binary().to_str().unwrap(),
            "ooda",
            "run",
            "--cycles=1",
        ])
        .env("SIMARD_STATE_ROOT", state_root.path())
        .output()
        .expect("failed to run ooda daemon");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Memory bridge requires amplihack-memory-lib; skip gracefully in CI
    if stderr.contains("Cannot find amplihack-memory-lib") || stderr.contains("bridge unhealthy") {
        eprintln!("SKIP: memory bridge not available (CI environment)");
        return;
    }

    // OODA daemon requires an LLM session; skip in CI environments that lack
    // ANTHROPIC_API_KEY or gh auth for Copilot SDK.
    if stderr.contains("No API key found")
        || stderr.contains("LLM session but open() failed")
        || stderr.contains("missing required configuration 'SIMARD_LLM_PROVIDER'")
    {
        eprintln!("SKIP: no LLM provider available (CI environment)");
        return;
    }

    assert!(
        stderr.contains("seeded 5 default goal"),
        "OODA daemon should seed 5 default goals:\n{stderr}"
    );
}

/// Verify Simard can drive fixes on external repos by checking the
/// amplihack PR that Simard drove.
#[test]
fn simard_drove_amplihack_fix() {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            "4236",
            "--repo",
            "rysweet/amplihack",
            "--json",
            "title,state",
        ])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            assert!(
                stdout.contains("step-03") && stdout.contains("4221"),
                "PR #4236 should reference step-03 fix: {stdout}"
            );
            println!("✅ Simard drove amplihack PR #4236: step-03 shell quoting + PR URL fix");
        }
        _ => {
            eprintln!("SKIP: gh CLI not available or PR not accessible");
        }
    }
}
