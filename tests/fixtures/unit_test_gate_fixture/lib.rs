//! Minimal fixture crate for the hermetic unit-test canary gate regression
//! tests (rysweet/Simard#4558).
//!
//! This crate is deliberately **not** part of the `simard` package build. The
//! `unit-test` canary gate ([`simard::self_relaunch::verify_canary`] driving
//! [`RelaunchGate::UnitTest`]) compiles and runs it in isolation via
//! `--manifest-path`, exactly as it runs the real tree. A regression test
//! (`tests/self_relaunch_hermetic_unit_test_gate.rs`) points the gate's
//! `manifest_dir` at this crate to prove, without a recursive full-suite run:
//!
//!   * **green:** with the toggle unset, both tests pass, so the gate goes
//!     GREEN even when a simulated live daemon holds the shared
//!     `SIMARD_STATE_ROOT` (proving the hermetic per-run temp state root, and
//!     that the `CARGO_HOME`/`RUSTUP_HOME` toolchain pin survives the `HOME`
//!     override); and
//!   * **red:** with the toggle set, [`tests::fixture_panics_when_toggled`]
//!     panics, so the gate goes RED and its `failing_detail` must carry the
//!     failing test **name** (not a truncated spinner fragment).
//!
//! The SAME crate serves as both the green and the red tree via a runtime
//! toggle (`SIMARD_GATE_FIXTURE_FAIL`) rather than a compile-time feature,
//! because the gate invokes a plain `cargo test` with no extra `--features`.
//! The toggle reaches the child through the gate's `canary_env` allow-list.

/// Trivial exported item so the fixture lib is never an empty crate. Not used
/// by the gate; present only to keep the fixture a well-formed `[lib]` target.
pub fn fixture_marker() -> u8 {
    42
}

#[cfg(test)]
mod tests {
    /// The green half: always passes. Present so a "green tree" run has at
    /// least one genuinely passing test alongside the (untriggered) toggle.
    #[test]
    fn fixture_passes_cleanly() {
        assert_eq!(super::fixture_marker(), 42);
    }

    /// The red half: panics **only** when `SIMARD_GATE_FIXTURE_FAIL` is present
    /// in the environment. The panic message and this test's fully-qualified
    /// name are what the gate's `extract_failure_detail` must surface into
    /// `failing_detail` on a red tree. With the toggle unset this is a no-op,
    /// so the same crate is a clean green tree.
    #[test]
    fn fixture_panics_when_toggled() {
        if std::env::var_os("SIMARD_GATE_FIXTURE_FAIL").is_some() {
            panic!("intentional fixture failure for red-canary detail extraction");
        }
    }
}
