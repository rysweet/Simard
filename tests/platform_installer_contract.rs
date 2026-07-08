//! Tests-first (TDD) contract for the Simard side of the platform installer
//! (issue #3119).
//!
//! Two Simard-owned behaviors are pinned here BEFORE they are implemented, so
//! these tests are expected to be **red** until the implementation step lands:
//!
//!   1. `simard ensure-deps` must stop probing for the Python `kuzu` package.
//!      Simard's cognitive memory is embedded **lbug** (compiled into the binary
//!      via `amplihack-memory-lib`), not kuzu. The stale kuzu check is misleading
//!      noise and must be removed; `ensure-deps` stays a minimal runtime check
//!      (`git`, `python3`, `gh`). See docs/reference/platform-installer-cli.md
//!      ("`simard ensure-deps` and the removed kuzu check").
//!
//!   2. Simard exposes a thin `simard platform install` / `simard platform
//!      doctor` rail that forwards to the canonical Crocutus scaffold. The verb
//!      is namespaced under `simard platform …` specifically to avoid colliding
//!      with the pre-existing `simard install` (binary-persist) verb. See
//!      docs/concepts/platform-installer.md ("Where the installer lives").
//!
//! These tests only exercise side-effect-free paths (`--help`, arg validation,
//! and the read-only `ensure-deps` report). They never invoke the installer's
//! host-mutating phases.

use std::process::Command;

use simard::dispatch_operator_cli;

/// Build an owned `Vec<String>` arg list for `dispatch_operator_cli`.
fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_string()).collect()
}

// ── 1. The stale kuzu dependency check is gone ──────────────────────────────

/// `simard ensure-deps` must NOT mention kuzu anywhere in its output. Memory is
/// embedded lbug, not kuzu; the probe is stale and misleading.
#[test]
fn ensure_deps_does_not_probe_for_kuzu() {
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("ensure-deps")
        .output()
        .expect("`simard ensure-deps` should launch");

    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !rendered.to_lowercase().contains("kuzu"),
        "`simard ensure-deps` must not reference the stale Python `kuzu` package \
         (memory is embedded lbug via amplihack-memory-lib):\n{rendered}"
    );
}

/// `ensure-deps` must still report the real minimal runtime dependencies.
#[test]
fn ensure_deps_still_reports_the_minimal_runtime_dependencies() {
    let output = Command::new(env!("CARGO_BIN_EXE_simard"))
        .arg("ensure-deps")
        .output()
        .expect("`simard ensure-deps` should launch");

    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    for dep in ["git", "python3", "gh"] {
        assert!(
            rendered.contains(dep),
            "`simard ensure-deps` should still check the minimal runtime dependency \
             '{dep}':\n{rendered}"
        );
    }
}

// ── 2. The `simard platform` rail exists and is namespaced ──────────────────

/// `simard platform --help` must be a recognized command that prints help and
/// exits Ok — not the current `unsupported command 'platform'` error.
#[test]
fn platform_group_help_is_recognized() {
    let result = dispatch_operator_cli(argv(&["platform", "--help"]));
    assert!(
        result.is_ok(),
        "`simard platform --help` should be recognized and print help, got: {:?}",
        result.err().map(|e| e.to_string())
    );
}

/// `simard platform install --help` must be recognized (the install rail).
#[test]
fn platform_install_help_is_recognized() {
    let result = dispatch_operator_cli(argv(&["platform", "install", "--help"]));
    assert!(
        result.is_ok(),
        "`simard platform install --help` should be recognized, got: {:?}",
        result.err().map(|e| e.to_string())
    );
}

/// `simard platform doctor --help` must be recognized (the preflight rail).
#[test]
fn platform_doctor_help_is_recognized() {
    let result = dispatch_operator_cli(argv(&["platform", "doctor", "--help"]));
    assert!(
        result.is_ok(),
        "`simard platform doctor --help` should be recognized, got: {:?}",
        result.err().map(|e| e.to_string())
    );
}

/// An unknown `platform` subcommand must fail closed with an error that names
/// the offending subcommand (proving the arg actually reached the `platform`
/// dispatcher rather than falling through the top-level `unsupported command
/// 'platform'` arm).
#[test]
fn platform_unknown_subcommand_fails_closed_naming_the_subcommand() {
    let result = dispatch_operator_cli(argv(&["platform", "totally-unknown-subcommand"]));
    let err = result.expect_err("an unknown `platform` subcommand must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("totally-unknown-subcommand"),
        "the error should name the unknown `platform` subcommand (proving it was \
         routed into the platform rail), got: {msg}"
    );
}

/// The pre-existing bare `simard install` (binary-persist) verb must keep
/// working and must NOT be shadowed by the new `platform` group — this guards
/// the verb-deconfliction decision. `install --help` is side-effect-free.
#[test]
fn preexisting_install_verb_is_not_shadowed() {
    let result = dispatch_operator_cli(argv(&["install", "--help"]));
    assert!(
        result.is_ok(),
        "the existing `simard install` verb must still work alongside `simard \
         platform install`, got: {:?}",
        result.err().map(|e| e.to_string())
    );
}
