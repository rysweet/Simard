//! Regression tests for the self-deploy crash-loop (issue #4693).
//!
//! SIGNATURE: every self-deploy of `rysweet/Simard` failed with
//! `deploy_gate: red canary (gate unit-test: tests failed (exit status: 101)):
//! failing tests: gym_list_shows_all_scenarios, meeting_repl_shows_greeting`,
//! recurring across 13+ distinct deploy commits over ~18h and blocking every
//! self-deploy (DeployDrift grew 1 -> 13 commits behind main).
//!
//! ROOT CAUSE: the `simard_binary()` helper in
//! `tests/e2e_engineer_external_repo.rs` resolved the binary from a hardcoded
//! `env!("CARGO_MANIFEST_DIR")` + `<target>/<debug>/simard` join. The
//! self-deploy deploy gate compiles and runs the suite inside a *redirected*
//! workspace (source `~/.simard/self-deploy-src`, target
//! `~/.simard/self-deploy-target`), so the manifest-relative binary path does
//! not exist there and the helper's `assert!(binary.exists(), ...)` panics
//! with `exit status: 101`, turning the canary red on every deploy.
//!
//! FIX: resolve the binary via Cargo's compile-time `CARGO_BIN_EXE_simard`,
//! which always points at the binary under the *active* `CARGO_TARGET_DIR`.
//!
//! These tests are TDD specs written before the fix:
//!   * [`no_hardcoded_manifest_target_binary_path_in_tests`] is a fast,
//!     always-run guard that fails while any hardcoded manifest-relative
//!     binary join remains in `tests/`. RED before the fix, GREEN after.
//!   * [`e2e_binary_resolution_survives_redirected_target_dir`] faithfully
//!     reproduces the deploy-gate environment by recompiling and running the
//!     two failing e2e tests under a redirected `CARGO_TARGET_DIR`. RED before
//!     the fix (exit 101), GREEN after. It is `#[ignore]`d because it performs
//!     a full from-scratch rebuild into a fresh target dir (expensive); when
//!     force-run it fails loudly rather than skipping (issue #2047).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The workspace root (crate manifest directory).
fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The forbidden hardcoded binary path fragment, assembled at runtime so this
/// guard file does not match its own scan.
fn forbidden_fragment() -> String {
    // Equivalent to the literal "target/debug/simard" that must not appear as
    // a manifest-relative join in the test sources.
    format!("target/{}/{}", "debug", "simard")
}

/// Recursively collect `.rs` files under `dir`.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read test dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("failed to read dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// FAST GUARD (always runs): no integration test may hardcode the
/// manifest-relative `target/debug/simard` path. Binaries must be resolved via
/// Cargo's compile-time `CARGO_BIN_EXE_simard`, which honors a redirected
/// `CARGO_TARGET_DIR`.
///
/// RED before the fix: `tests/e2e_engineer_external_repo.rs` contains the
/// hardcoded join (and a stale doc-comment referencing it).
/// GREEN after the fix: the helper and doc-comment no longer reference it.
#[test]
fn no_hardcoded_manifest_target_binary_path_in_tests() {
    let tests_dir = manifest_dir().join("tests");
    let mut files = Vec::new();
    collect_rs_files(&tests_dir, &mut files);
    assert!(
        !files.is_empty(),
        "expected to find test sources under {}",
        tests_dir.display()
    );

    let needle = forbidden_fragment();
    let this_file = Path::new(file!())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let mut offenders = Vec::new();
    for file in &files {
        // Skip this guard file itself (it never contains the literal).
        if file.file_name().and_then(|n| n.to_str()) == Some(this_file) {
            continue;
        }
        let contents = std::fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", file.display()));
        for (idx, line) in contents.lines().enumerate() {
            if line.contains(&needle) {
                offenders.push(format!("{}:{}: {}", file.display(), idx + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "Found hardcoded manifest-relative binary path(s) in tests/. These break \
         under a redirected CARGO_TARGET_DIR (the self-deploy canary) and caused \
         the deploy_gate exit-101 crash-loop (issue #4693). Resolve binaries via \
         env!(\"CARGO_BIN_EXE_simard\") instead:\n{}",
        offenders.join("\n")
    );
}

/// FAITHFUL REPRODUCTION (`#[ignore]`d — expensive full rebuild): recompile and
/// run the two deploy-gate-failing e2e tests under a redirected
/// `CARGO_TARGET_DIR`, exactly as the self-deploy canary does.
///
/// RED before the fix: the e2e helper looks for the binary under
/// `<manifest>/target/debug/simard`, which does not exist in the redirected
/// target dir, so the `assert!(binary.exists())` guard panics and `cargo test`
/// exits non-zero (the `exit status: 101` seen in the OODA journal).
/// GREEN after the fix: `CARGO_BIN_EXE_simard` points into the redirected
/// target dir, the binary is found, and both tests pass.
#[test]
#[ignore = "expensive: performs a full from-scratch rebuild into a redirected \
            CARGO_TARGET_DIR to reproduce the self-deploy canary; force-run for \
            issue #4693 verification. Fails loudly rather than skipping (issue #2047)."]
fn e2e_binary_resolution_survives_redirected_target_dir() {
    let root = manifest_dir();

    // A redirected target dir distinct from <manifest>/target, mirroring the
    // self-deploy canary's `~/.simard/self-deploy-target`.
    let redirected = tempfile::tempdir().expect("temp dir for redirected CARGO_TARGET_DIR");
    let redirected_target = redirected.path().join("self-deploy-target");
    assert_ne!(
        redirected_target,
        root.join("target"),
        "redirected target dir must differ from the manifest target dir to \
         reproduce the canary"
    );

    let output = Command::new(env!("CARGO"))
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", &redirected_target)
        .args([
            "test",
            "--test",
            "e2e_engineer_external_repo",
            "--",
            "gym_list_shows_all_scenarios",
            "meeting_repl_shows_greeting",
        ])
        .output()
        .expect("failed to spawn `cargo test` under redirected CARGO_TARGET_DIR");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "The e2e tests gym_list_shows_all_scenarios and meeting_repl_shows_greeting \
         must pass under a redirected CARGO_TARGET_DIR (the self-deploy canary \
         environment). A failure here is the issue #4693 deploy_gate crash-loop: \
         the binary must be resolved via CARGO_BIN_EXE_simard, not a \
         manifest-relative path.\n\
         --- redirected CARGO_TARGET_DIR: {}\n\
         --- cargo status: {}\n\
         --- stdout ---\n{}\n--- stderr ---\n{}",
        redirected_target.display(),
        output.status,
        stdout,
        stderr,
    );
}
