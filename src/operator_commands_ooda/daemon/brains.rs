//! Brain construction for the OODA daemon.
//!
//! Centralises wire-up of the three prompt-driven OODA brains:
//!   * [`crate::ooda_brain::OodaBrain`] — engineer-lifecycle (PR #1458, act phase).
//!   * [`crate::ooda_brain::OodaDecideBrain`] — decide phase (PR #1469).
//!   * [`crate::ooda_brain::OodaOrientBrain`] — orient phase (PR #1471).
//!
//! Each builder falls back to a deterministic brain when no LLM provider is
//! configured. Per operator hard constraint (issues #1711, #1748), that
//! degradation **must be LOUD**: every fallback construction logs at ERROR
//! severity, writes a dashboard-visible line to `ooda.log`, and increments
//! a process-wide counter (see [`fallback_brain_count`]) so operators
//! cannot miss that the daemon is running in degraded mode.
//!
//! This is the first installment of the #1748 ladder. Step 2 will convert
//! the fallback brains' `judge_*` methods to return `Err` instead of
//! `Ok(safe_default)`. Step 3 will delete the fallback brains entirely
//! and replace the engineer-lifecycle brain with an agentic subprocess
//! per #1711's spec.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::helpers::daemon_log;

/// Counts how many times any `Deterministic*FallbackBrain` has been
/// constructed in this process. Bumped from [`record_fallback`].
///
/// Exposed via [`fallback_brain_count`] for tests + a future
/// `/api/health` endpoint that should refuse "healthy" when this is
/// nonzero. The metric is process-wide (not per-phase) on purpose: the
/// loudest possible signal is "you're degraded somewhere" — the log
/// line carries the per-phase detail.
static FALLBACK_BRAIN_COUNT: AtomicU64 = AtomicU64::new(0);

/// Read the process-wide count of fallback-brain constructions.
pub fn fallback_brain_count() -> u64 {
    FALLBACK_BRAIN_COUNT.load(Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn reset_fallback_brain_count_for_test() {
    FALLBACK_BRAIN_COUNT.store(0, Ordering::Relaxed);
}

/// Loudly record that a fallback brain was constructed: ERROR-level
/// tracing, dashboard-visible `ooda.log` line, and a process-wide
/// counter bump.
///
/// `phase` is the OODA phase whose brain fell back (`"act"`,
/// `"decide"`, or `"orient"`). `reason` is the `LlmProvider::resolve()`
/// error rendered to string.
fn record_fallback(state_root: &Path, phase: &str, reason: &str) {
    FALLBACK_BRAIN_COUNT.fetch_add(1, Ordering::Relaxed);
    tracing::error!(
        target: "simard::ooda::fallback",
        phase = phase,
        reason = reason,
        "OODA daemon running in DEGRADED mode: {phase}_brain fell back to deterministic (no LLM available) — see issues #1711, #1748",
    );
    daemon_log(
        state_root,
        &format!(
            "[simard] OODA daemon: DEGRADED — {phase}_brain = Deterministic*FallbackBrain (no LLM: {reason}); see issues #1711, #1748"
        ),
    );
}

/// Construct the engineer-lifecycle brain. Always returns an Arc — falls
/// back to [`crate::ooda_brain::DeterministicFallbackBrain`] on Err,
/// loudly per [`record_fallback`].
pub(super) fn build_act_brain(
    state_root: &Path,
    repo_root: &Path,
) -> Arc<dyn crate::ooda_brain::OodaBrain> {
    // Try recipe brain first (recipe-runner-rs backed)
    if let Some(b) = crate::ooda_brain::RecipeEngineerLifecycleBrain::new(repo_root) {
        daemon_log(
            state_root,
            "[simard] OODA daemon: brain = RecipeEngineerLifecycleBrain (recipe-runner-rs backed)",
        );
        return Arc::new(b);
    }
    // Fall back to LLM-backed brain
    match crate::ooda_brain::build_rustyclawd_brain() {
        Ok(b) => {
            daemon_log(
                state_root,
                "[simard] OODA daemon: brain = RustyClawdBrain (prompt-driven)",
            );
            b.into()
        }
        Err(e) => {
            record_fallback(state_root, "act", &e.to_string());
            Arc::new(crate::ooda_brain::DeterministicFallbackBrain)
        }
    }
}

/// Construct the Decide brain (issue #2111 — recipe-runner-rs backed).
/// Returns `None` when recipe-runner-rs is unavailable so
/// `cycle::run_ooda_cycle` falls back to [`crate::ooda_brain::DeterministicFallbackDecideBrain`].
pub(super) fn build_decide_brain(
    state_root: &Path,
    repo_root: &Path,
) -> Option<Arc<dyn crate::ooda_brain::OodaDecideBrain>> {
    match crate::ooda_brain::RecipeDecideBrain::new(repo_root) {
        Some(b) => {
            daemon_log(
                state_root,
                "[simard] OODA daemon: decide_brain = RecipeDecideBrain (recipe-runner-rs backed)",
            );
            Some(Arc::new(b))
        }
        None => {
            record_fallback(
                state_root,
                "decide",
                "recipe-runner-rs or ooda-decide.yaml not available",
            );
            None
        }
    }
}

/// Construct the Orient brain (PR #1471 wire-up). Same pattern as
/// [`build_decide_brain`].
pub(super) fn build_orient_brain(
    state_root: &Path,
    repo_root: &Path,
) -> Option<Arc<dyn crate::ooda_brain::OodaOrientBrain>> {
    // Try recipe brain first (recipe-runner-rs backed)
    if let Some(b) = crate::ooda_brain::RecipeOrientBrain::new(repo_root) {
        daemon_log(
            state_root,
            "[simard] OODA daemon: orient_brain = RecipeOrientBrain (recipe-runner-rs backed)",
        );
        return Some(Arc::new(b));
    }
    // Fall back to LLM-backed brain
    match crate::ooda_brain::build_rustyclawd_orient_brain() {
        Ok(b) => {
            daemon_log(
                state_root,
                "[simard] OODA daemon: orient_brain = RustyClawdOrientBrain (prompt-driven)",
            );
            Some(Arc::from(b))
        }
        Err(e) => {
            record_fallback(state_root, "orient", &e.to_string());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Serializes tests that mutate process-global state (env vars + the
    // atomic counter). cargo's parallel runner would otherwise interleave
    // record_fallback() calls and corrupt counter expectations.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static M: Mutex<()> = Mutex::new(());
        M.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Direct unit test: record_fallback bumps the counter and writes a
    /// dashboard-visible line containing the issue references. Does NOT
    /// depend on LlmProvider::resolve() at all — exercises the loud-
    /// failure recording code path in isolation.
    #[test]
    fn record_fallback_bumps_counter_and_writes_dashboard_log() {
        let _g = lock();
        reset_fallback_brain_count_for_test();
        let tmp = TempDir::new().expect("tempdir");

        assert_eq!(fallback_brain_count(), 0, "precondition");
        record_fallback(tmp.path(), "act", "no llm provider configured");
        assert_eq!(
            fallback_brain_count(),
            1,
            "counter must increment on every fallback construction"
        );

        let log =
            std::fs::read_to_string(tmp.path().join("ooda.log")).expect("ooda.log must be created");
        assert!(
            log.contains("DEGRADED"),
            "log must contain DEGRADED marker; got: {log}"
        );
        assert!(
            log.contains("act_brain"),
            "log must name the phase; got: {log}"
        );
        assert!(
            log.contains("#1711") && log.contains("#1748"),
            "log must cite tracking issues so operators can find context; got: {log}"
        );
    }

    /// Counter is monotonic across multiple fallback constructions
    /// regardless of phase.
    #[test]
    fn record_fallback_counter_accumulates_across_phases() {
        let _g = lock();
        reset_fallback_brain_count_for_test();
        let tmp = TempDir::new().expect("tempdir");

        record_fallback(tmp.path(), "act", "x");
        record_fallback(tmp.path(), "decide", "y");
        record_fallback(tmp.path(), "orient", "z");
        assert_eq!(fallback_brain_count(), 3);
    }

    /// The reason string from LlmProvider::resolve() must be preserved
    /// verbatim in the dashboard log so operators can diagnose without
    /// hunting through tracing output.
    #[test]
    fn record_fallback_preserves_resolve_error_in_dashboard_log() {
        let _g = lock();
        reset_fallback_brain_count_for_test();
        let tmp = TempDir::new().expect("tempdir");
        let unique_reason = "config.toml missing llm_provider key (unit-test sentinel)";

        record_fallback(tmp.path(), "decide", unique_reason);

        let log = std::fs::read_to_string(tmp.path().join("ooda.log")).expect("log");
        assert!(
            log.contains(unique_reason),
            "the LlmProvider::resolve() error must be in the dashboard log verbatim; got: {log}"
        );
    }
}
