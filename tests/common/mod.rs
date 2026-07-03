//! Shared test support for integration tests that drive the full engineer
//! loop, which requires a real LLM provider/session.
//!
//! Issue #2047: integration tests must never *silently pass by skipping*.
//! The old `skip_if_no_llm_provider` helper returned early (`return;`) when no
//! LLM provider was available, so a test that never actually ran was still
//! reported by the harness as `ok` — a green that lies.
//!
//! The fix has two layers:
//!   1. Tests that require an LLM provider are marked `#[ignore]`, so the
//!      default `cargo test` run reports them honestly as `ignored` (visible)
//!      instead of counting them as `passed`.
//!   2. When such a test is *explicitly* force-run (e.g.
//!      `cargo test -- --include-ignored`) and the provider is genuinely
//!      unavailable, [`require_llm_provider`] panics with a clear, actionable
//!      message instead of returning early. A skipped run can therefore never
//!      masquerade as a pass.
//!
//! This module is included via `mod common;` in each integration-test crate
//! that needs it. Not every crate uses every helper, hence `allow(dead_code)`.
//!
//! Unit tests for these helpers live in `tests/engineer_loop.rs` so they run
//! once rather than once per including crate.

#![allow(dead_code)]

/// Returns true when the rendered probe/CLI output indicates that no usable
/// LLM provider/session was available.
///
/// Covers the missing-config error surfaced by `RuntimeConfig::load()` when
/// `SIMARD_LLM_PROVIDER` is unset, legacy "no API key"/session-open failures,
/// and the `amplihack RustyClawd` subprocess failures that occur after the
/// engineer-loop subprocess pivot (issue #1648) when CI has no auth/network.
pub fn llm_provider_unavailable(rendered: &str) -> bool {
    rendered.contains("No API key found")
        || rendered.contains("LLM-based review is unavailable")
        || rendered.contains("LLM session but open() failed")
        || rendered.contains("base type 'review-pipeline-rustyclawd' failed")
        || rendered.contains("missing required configuration 'SIMARD_LLM_PROVIDER'")
        // After the engineer-loop subprocess pivot (issue #1648), the loop
        // shells out to `amplihack RustyClawd --auto`. CI cannot complete a
        // real RustyClawd run (no auth, no network for the LLM provider).
        || rendered.contains("amplihack RustyClawd")
        || rendered.contains("RustyClawd exited with status")
        || rendered.contains("failed to spawn `amplihack")
        || rendered.contains("agent session failed")
}

/// Returns true when the rendered output indicates the amplihack memory adapter
/// (`amplihack-memory-lib`) is unavailable or unhealthy.
///
/// The OODA daemon opens this adapter at startup; CI hosts that lack the native
/// library surface the "Cannot find amplihack-memory-lib"/"adapter unhealthy"
/// errors instead of seeding goals.
pub fn memory_adapter_unavailable(rendered: &str) -> bool {
    rendered.contains("Cannot find amplihack-memory-lib") || rendered.contains("adapter unhealthy")
}

/// Fail-explicit guard for `#[ignore]`d tests that require the amplihack memory
/// adapter. Mirrors [`require_llm_provider`] (issue #2047): when such a test is
/// force-run (`cargo test -- --include-ignored`) but the adapter is unavailable,
/// this panics with an actionable message so the run fails loudly instead of
/// silently passing by an early `return`.
pub fn require_memory_adapter(test_name: &str, rendered: &str) {
    if memory_adapter_unavailable(rendered) {
        panic!(
            "{test_name}: requires the amplihack memory adapter (amplihack-memory-lib), \
             but it is unavailable or unhealthy.\n\
             This test is `#[ignore]`d by default; you force-ran it (e.g. \
             `cargo test -- --include-ignored`). Provision the memory adapter then retry, \
             or run the default `cargo test`, which honestly reports this test as `ignored` \
             rather than passing it by skipping. See issue #2047.\n\
             --- rendered probe output ---\n{rendered}"
        );
    }
}

/// Fail-explicit guard for the `#[ignore]`d integration tests that drive the
/// full engineer loop and therefore require a real LLM provider/session.
///
/// These tests are `#[ignore]`d by default, so the standard `cargo test` run
/// reports them as `ignored` rather than `passed` (issue #2047). When a caller
/// *force-runs* them (`cargo test -- --include-ignored`) but no provider is
/// available, this panics with an actionable message so the run fails loudly
/// instead of silently passing.
///
/// Call this after capturing the probe's rendered output and before asserting
/// on provider-dependent behaviour.
pub fn require_llm_provider(test_name: &str, rendered: &str) {
    if llm_provider_unavailable(rendered) {
        panic!(
            "{test_name}: requires a real LLM provider/session, but none is available.\n\
             This test is `#[ignore]`d by default; you force-ran it (e.g. \
             `cargo test -- --include-ignored`). Configure SIMARD_LLM_PROVIDER \
             (and the matching auth) then retry, or run the default `cargo test`, \
             which honestly reports this test as `ignored` rather than passing it \
             by skipping. See issue #2047.\n\
             --- rendered probe output ---\n{rendered}"
        );
    }
}
