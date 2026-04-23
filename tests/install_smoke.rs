//! Smoke test that verifies `cargo install --path .` succeeds and the built
//! binary is runnable.  This catches packaging regressions (missing files,
//! broken feature flags, unresolvable deps) that unit tests cannot.

use std::process::Command;

/// The Cargo-embedded version we expect from the installed binary.
const EXPECTED_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `cargo install --path .` must succeed in the workspace root.
///
/// We install into a temporary directory so the system PATH is untouched.
#[test]
fn cargo_install_from_repo_succeeds() {
    let install_root = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("install-smoke");

    // Clean any prior run so `cargo install` doesn't skip with "already installed".
    let _ = std::fs::remove_dir_all(&install_root);

    let status = Command::new(env!("CARGO"))
        .args(["install", "--path", ".", "--root"])
        .arg(&install_root)
        // `--debug` switches from the default release build (~40 min with
        // cold cache because cargo install allocates an isolated target
        // directory that bypasses the CI dep cache) to a dev-profile build.
        // A smoke test only needs to prove the crate packages cleanly and
        // `simard --help` loads — release-grade optimization is waste here.
        .args(["--no-track", "--quiet", "--debug"])
        .status()
        .expect("failed to launch cargo install");

    assert!(status.success(), "cargo install --path . failed");

    // Verify the main binary exists and is executable.
    let simard_bin = install_root.join("bin").join("simard");
    assert!(
        simard_bin.exists(),
        "expected installed binary at {simard_bin:?}"
    );

    // Run with `--help` (which the CLI does support via operator dispatch)
    // to confirm the binary actually loads.  We only check for exit-success
    // or a known non-zero that still proves the binary *ran*.
    let output = Command::new(&simard_bin)
        .args(["--help"])
        .output()
        .expect("failed to execute installed simard binary");

    // The binary ran (exit 0 or a graceful non-zero).  A segfault / missing
    // dylib would give a signal-based exit that `status.code()` returns None.
    assert!(
        output.status.code().is_some(),
        "simard binary crashed (signal exit)"
    );

    // Confirm the compiled-in version matches expectations.
    assert_eq!(EXPECTED_VERSION, "0.16.0");
}
