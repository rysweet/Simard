//! Brain construction for the OODA daemon.
//!
//! Centralises wire-up of the three prompt-driven OODA brains:
//!   * [`crate::ooda_brain::OodaBrain`] — engineer-lifecycle (PR #1458, act phase).
//!   * [`crate::ooda_brain::OodaDecideBrain`] — decide phase (PR #1469).
//!   * [`crate::ooda_brain::OodaOrientBrain`] — orient phase (PR #1471).
//!
//! Each builder degrades gracefully: on `Err` (no LLM provider configured,
//! adapter init failure, etc.) the daemon proceeds with the deterministic
//! fallback so the cycle never depends on LLM availability.

use std::path::Path;
use std::sync::Arc;

use super::helpers::daemon_log;

/// Construct the engineer-lifecycle brain. Always returns an Arc — falls
/// back to [`crate::ooda_brain::DeterministicFallbackBrain`] on Err.
pub(super) fn build_act_brain(state_root: &Path) -> Arc<dyn crate::ooda_brain::OodaBrain> {
    match crate::ooda_brain::build_rustyclawd_brain() {
        Ok(b) => {
            daemon_log(
                state_root,
                "[simard] OODA daemon: brain = RustyClawdBrain (prompt-driven)",
            );
            b.into()
        }
        Err(e) => {
            daemon_log(
                state_root,
                &format!(
                    "[simard] OODA daemon: brain = DeterministicFallbackBrain (rustyclawd unavailable: {e})"
                ),
            );
            Arc::new(crate::ooda_brain::DeterministicFallbackBrain)
        }
    }
}

/// Construct the Decide brain (PR #1469 wire-up). Returns `None` on Err so
/// `cycle::run_ooda_cycle` falls back to [`crate::ooda_brain::DeterministicFallbackDecideBrain`].
pub(super) fn build_decide_brain(
    state_root: &Path,
) -> Option<Arc<dyn crate::ooda_brain::OodaDecideBrain>> {
    match crate::ooda_brain::build_rustyclawd_decide_brain() {
        Ok(b) => {
            daemon_log(
                state_root,
                "[simard] OODA daemon: decide_brain = RustyClawdDecideBrain (prompt-driven)",
            );
            Some(Arc::from(b))
        }
        Err(e) => {
            daemon_log(
                state_root,
                &format!(
                    "[simard] OODA daemon: decide_brain = DeterministicFallbackDecideBrain (no LLM: {e})"
                ),
            );
            None
        }
    }
}

/// Construct the Orient brain (PR #1471 wire-up). Same pattern as
/// [`build_decide_brain`].
pub(super) fn build_orient_brain(
    state_root: &Path,
) -> Option<Arc<dyn crate::ooda_brain::OodaOrientBrain>> {
    match crate::ooda_brain::build_rustyclawd_orient_brain() {
        Ok(b) => {
            daemon_log(
                state_root,
                "[simard] OODA daemon: orient_brain = RustyClawdOrientBrain (prompt-driven)",
            );
            Some(Arc::from(b))
        }
        Err(e) => {
            daemon_log(
                state_root,
                &format!(
                    "[simard] OODA daemon: orient_brain = DeterministicFallbackOrientBrain (no LLM: {e})"
                ),
            );
            None
        }
    }
}
