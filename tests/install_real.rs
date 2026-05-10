//! End-to-end install verification.
//!
//! This is *not* a "smoke" test. Each step exercises real install
//! production code paths and asserts a concrete observable outcome
//! against the binary it just installed:
//!
//!   1. `cargo install --path .` packages the crate into a tempdir.
//!   2. The installed binary's `--version` output is parsed and matches
//!      `CARGO_PKG_VERSION` (catches any drift between Cargo metadata
//!      and the runtime CLI dispatch — the previous test compared two
//!      compile-time constants which proved nothing).
//!   3. `simard ensure-deps` runs against the installed binary and
//!      exits 0 (proves the dep-probing logic is reachable from the
//!      packaged binary, not just from `cargo run`).
//!   4. `simard install` from the packaged binary copies itself into
//!      an isolated `$HOME/.simard/bin/simard` and the resulting
//!      second-hop binary is itself runnable and reports the same
//!      version. This exercises the actual install command users run.
//!
//! Failure of any step indicates a real packaging or runtime regression.

use std::path::Path;
use std::process::Command;

const EXPECTED_VERSION: &str = env!("CARGO_PKG_VERSION");

#[test]
fn install_packages_runs_and_self_installs() {
    let workspace = std::env::current_dir().expect("cwd");
    let install_root = workspace.join("target").join("install-real");

    // Clean any prior run so `cargo install` doesn't skip with
    // "already installed".
    let _ = std::fs::remove_dir_all(&install_root);

    // ── Step 1: cargo install ───────────────────────────────────────
    //
    // `--debug` switches to the dev profile so cold-cache install in
    // CI doesn't take ~40min building release. We're verifying the
    // packaging + entry points work, not optimization.
    let install_status = Command::new(env!("CARGO"))
        .args(["install", "--path", ".", "--root"])
        .arg(&install_root)
        .args(["--no-track", "--quiet", "--debug"])
        .status()
        .expect("failed to launch cargo install");
    assert!(install_status.success(), "cargo install --path . failed");

    let installed_simard = install_root.join("bin").join("simard");
    assert!(
        installed_simard.exists(),
        "expected installed binary at {installed_simard:?}"
    );

    // ── Step 2: --version against installed binary ──────────────────
    //
    // Parse the runtime version output and compare against the
    // crate's compile-time CARGO_PKG_VERSION. This is the actual
    // packaging-vs-CLI consistency check the prior test claimed to be.
    let version_output = run_capture(&installed_simard, &["--version"]);
    let version_line = version_output.trim();
    assert_eq!(
        version_line,
        format!("simard {EXPECTED_VERSION}"),
        "installed simard --version returned {version_line:?}, expected 'simard {EXPECTED_VERSION}'"
    );

    // ── Step 3: ensure-deps actually runs against installed binary ──
    //
    // ensure-deps is the runtime probe for required tools (git,
    // python3, gh) and the optional kuzu Python package. It's the
    // first real subcommand any operator runs after install. If the
    // packaged binary can't reach this code path, the install is
    // broken even if --version works.
    let ensure_status = Command::new(&installed_simard)
        .arg("ensure-deps")
        .status()
        .expect("failed to launch installed simard ensure-deps");
    assert!(
        ensure_status.success(),
        "installed simard ensure-deps failed (exit {:?})",
        ensure_status.code()
    );

    // ── Step 4: simard install (the user-facing install path) ───────
    //
    // The `install` subcommand is what end users run after extracting
    // a release tarball — it copies the running binary to
    // $HOME/.simard/bin/simard and re-runs ensure-deps. We give it an
    // isolated HOME so we don't pollute the developer's real
    // ~/.simard directory.
    let fake_home = install_root.join("fake-home");
    std::fs::create_dir_all(&fake_home).expect("create fake home");
    let self_install_status = Command::new(&installed_simard)
        .arg("install")
        .env("HOME", &fake_home)
        .status()
        .expect("failed to launch installed simard install");
    assert!(
        self_install_status.success(),
        "installed simard install (HOME={}) failed",
        fake_home.display()
    );

    let second_hop = fake_home.join(".simard").join("bin").join("simard");
    assert!(
        second_hop.exists(),
        "second-hop binary missing at {second_hop:?}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&second_hop)
            .expect("stat second-hop")
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "second-hop binary at {second_hop:?} not executable (mode {mode:o})"
        );
    }

    // The second-hop binary must itself report the same version.
    // If `simard install` ever copied a stale or wrong binary, this
    // would catch it.
    let second_hop_version = run_capture(&second_hop, &["--version"]);
    assert_eq!(
        second_hop_version.trim(),
        format!("simard {EXPECTED_VERSION}"),
        "second-hop simard --version mismatch"
    );
}

fn run_capture(bin: &Path, args: &[&str]) -> String {
    let output = Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to execute {bin:?} {args:?}: {e}"));
    assert!(
        output.status.success(),
        "{bin:?} {args:?} exited {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}
