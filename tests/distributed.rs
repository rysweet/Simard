//! Integration tests for distributed Simard across VMs.
//!
//! These tests require the Simard VM to be reachable via `azlin connect`.
//! They are ignored by default and can be run with:
//!   cargo test --test distributed -- --ignored
//!
//! Environment: Simard VM (rysweet-linux-vm-pool) must be running
//! and accessible via bastion.

use std::process::Command;

/// Helper: run a command on the remote Simard VM via azlin connect.
/// Returns (stdout, stderr, success).
fn remote_cmd(cmd: &str) -> (String, String, bool) {
    let full_cmd = format!(
        "export PATH=\"$HOME/.cargo/bin:$HOME/.simard/bin:$PATH\" && \
         export CARGO_TARGET_DIR=/mnt/tmp-data/cargo-target && \
         cd ~/src/Simard && {cmd}"
    );

    let output = Command::new("azlin")
        .args([
            "connect",
            "Simard",
            "--resource-group",
            "rysweet-linux-vm-pool",
            "--no-tmux",
            "--",
            &full_cmd,
        ])
        .output()
        .expect("azlin connect failed to execute");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

/// Check if the Simard VM is reachable. Skip all tests if not.
fn require_vm() {
    let (stdout, _, ok) = remote_cmd("echo REACHABLE");
    if !ok || !stdout.contains("REACHABLE") {
        panic!("Simard VM not reachable — run these tests with a live VM");
    }
}

#[test]
#[ignore] // requires live VM
fn remote_simard_ensure_deps() {
    require_vm();
    let (stdout, _, ok) = remote_cmd("simard ensure-deps 2>&1");
    assert!(ok, "ensure-deps should succeed");
    assert!(
        stdout.contains("All dependencies satisfied"),
        "expected all deps satisfied: {stdout}"
    );
}

#[test]
#[ignore]
fn remote_simard_spawn_subordinate() {
    require_vm();
    let (stdout, _, ok) = remote_cmd(
        "timeout 10 simard spawn dist-test-agent \
         'echo hello from distributed agent' \
         \"$(pwd)\" --depth=0 2>&1",
    );
    assert!(ok, "spawn should succeed: {stdout}");
    assert!(
        stdout.contains("spawned subordinate 'dist-test-agent'"),
        "expected spawn confirmation: {stdout}"
    );
}

#[test]
#[ignore]
fn remote_simard_ooda_single_cycle() {
    require_vm();
    let (stdout, _, _) = remote_cmd(
        "STATE=$(mktemp -d /mnt/tmp-data/tmp/simard-dist-test.XXXXXX) && \
         timeout 30 simard ooda run --cycles=1 \"$STATE\" 2>&1 && \
         rm -rf \"$STATE\"",
    );
    assert!(
        stdout.contains("seeded 5 default goal"),
        "should seed goals on fresh state: {stdout}"
    );
    assert!(
        stdout.contains("OODA daemon: completed 1 cycle"),
        "should complete 1 cycle: {stdout}"
    );
}

#[test]
#[ignore]
fn remote_simard_test_suite_passes() {
    require_vm();
    let (stdout, _, ok) = remote_cmd("cargo test --lib cmd_ensure_deps -- -q 2>&1 | tail -5");
    assert!(ok, "tests should pass on VM: {stdout}");
    assert!(
        stdout.contains("4 passed"),
        "expected 4 cmd_ensure_deps tests: {stdout}"
    );
}

#[test]
#[ignore]
fn remote_simard_cleanup_runs() {
    require_vm();
    let (stdout, _, ok) = remote_cmd("simard cleanup 2>&1");
    assert!(ok, "cleanup should succeed: {stdout}");
    assert!(
        stdout.contains("Cleanup report"),
        "expected cleanup report: {stdout}"
    );
}
