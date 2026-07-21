//! The autonomous self-deploy TRIGGER (#2590): the thin deterministic rail that
//! connects deploy-drift OBSERVE to a guarded Deploy ACT.
//!
//! This module owns only the small, pure, hermetically-testable pieces of the
//! rail:
//!
//!   - [`deploy_drift_signal`] — turn a [`DeployDrift`] (from
//!     [`ReconcileDetector::detect`](crate::self_deploy::ReconcileDetector::detect),
//!     which is fail-safe: a git error reports "no drift") into a first-class
//!     [`Signal::DeployDriftDetected`]. Absent (`None`) whenever the daemon is
//!     current, so no drift ⇒ no signal ⇒ no deploy.
//!   - [`autonomous_deploy_enabled`] — the documented opt-OUT env
//!     (`SIMARD_OVERSEER_AUTONOMOUS_DEPLOY=0`) that lets an operator pin the
//!     daemon. Enabled by default (consistent with `build_overseer` already
//!     setting `high_risk_autonomy(true)`).
//!   - [`global_deploy_throttle_allow`] — the process-global anti-thrash
//!     min-interval guard so a persisting single-commit drift (re-observed every
//!     tick until the swap lands) can never make the daemon redeploy every
//!     cycle. Process-global (a `static`) because the daemon rebuilds the acting
//!     `Overseer` every tick, so per-instance state could never throttle.
//!
//! The guarded go/no-go SAFETY judgment (no-op / rollback / red-canary /
//! crash-loop refusals, canary build+verify, rollback, operator notification)
//! stays in [`crate::overseer::deploy::GuardedDeployer`] and the high-risk
//! `AutonomyGate` — this module never swaps a binary itself.

use crate::overseer::capabilities::DeployDriftObservation;
use crate::overseer::signal::Signal;
use crate::self_deploy::{
    DeployDrift, DeploySource, GitDeploySource, GitSourcePreparer, ReconcileDetector,
};

/// Opt-out env var: set to `0`/`false`/`off`/`no` to PIN the daemon (disable
/// autonomous drift-triggered self-deploy). Any other value (or unset) leaves it
/// ENABLED, consistent with `build_overseer`'s `high_risk_autonomy(true)`.
pub const AUTONOMOUS_DEPLOY_ENV: &str = "SIMARD_OVERSEER_AUTONOMOUS_DEPLOY";

/// Env var overriding the anti-thrash minimum interval (seconds) between
/// autonomous deploy attempts. Clamped to at least [`MIN_DEPLOY_INTERVAL_FLOOR`].
pub const DEPLOY_MIN_INTERVAL_ENV: &str = "SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS";

/// Default anti-thrash interval: 15 minutes. Long enough that a single merged
/// commit cannot make the daemon redeploy every tick, short enough that a real
/// backlog of merged work still lands promptly.
pub const DEFAULT_DEPLOY_INTERVAL_SECS: u64 = 900;

/// Hard floor for the anti-thrash interval so a mis-set env can never collapse
/// the churn guard to "every tick".
pub const MIN_DEPLOY_INTERVAL_FLOOR: u64 = 60;

/// Is autonomous drift-triggered self-deploy ENABLED? Default `true`; an operator
/// pins the daemon with `SIMARD_OVERSEER_AUTONOMOUS_DEPLOY=0` (also `false`/`off`/
/// `no`, case-insensitive). Fail-open: an unreadable/empty value stays enabled.
pub fn autonomous_deploy_enabled() -> bool {
    match std::env::var(AUTONOMOUS_DEPLOY_ENV) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => true,
    }
}

/// Build the [`Signal::DeployDriftDetected`] for the current Observe pass, or
/// `None` when there is nothing to deploy.
///
/// Fail-SAFE by construction: the caller passes the [`DeployDrift`] that
/// `ReconcileDetector::detect` already degraded to `needs_deploy: false` on any
/// git/source error, so an error simply yields `None` (no signal ⇒ no deploy).
/// Also `None` when the merged head could not be resolved to a non-empty commit
/// (never emit a deploy signal with no target — the deterministic Decide rail is
/// fail-closed on an empty commit).
pub fn deploy_drift_signal(drift: &DeployDrift, target_commit: &str) -> Option<Signal> {
    if !drift.needs_deploy {
        return None;
    }
    let target = target_commit.trim();
    if target.is_empty() {
        return None;
    }
    Some(Signal::DeployDriftDetected {
        target_commit: target.to_string(),
        behind_commits: drift.behind_commits,
    })
}

/// Resolve the anti-thrash minimum interval (seconds) from the environment,
/// clamped to [`MIN_DEPLOY_INTERVAL_FLOOR`]. An unset/unparseable value uses
/// [`DEFAULT_DEPLOY_INTERVAL_SECS`].
pub fn deploy_min_interval_secs() -> u64 {
    std::env::var(DEPLOY_MIN_INTERVAL_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_DEPLOY_INTERVAL_SECS)
        .max(MIN_DEPLOY_INTERVAL_FLOOR)
}

/// Epoch-seconds of the last autonomous deploy ATTEMPT this process recorded.
/// PROCESS-GLOBAL (not per-`Overseer`) deliberately: the daemon rebuilds the
/// acting `Overseer` — and therefore any per-instance throttle — from scratch on
/// EVERY tick (`build_overseer` in the tick thread), so a per-instance guard
/// would reset each tick and never actually throttle. A `static` survives across
/// ticks within the one long-lived daemon process, which is exactly the window a
/// persisting single-commit drift is re-observed in. `0` means "never attempted".
static LAST_DEPLOY_ATTEMPT_SECS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Process-global anti-thrash guard: may an autonomous deploy ATTEMPT proceed at
/// `now_secs`? Returns `true` and RECORDS the attempt when at least
/// `min_interval_secs` has elapsed since the last recorded attempt (or none has);
/// returns `false` WITHOUT recording while a prior attempt is still inside the
/// window. Recording on allow (not on later success) is deliberate:
/// even an attempt the guarded executor later refuses resets the window, so a
/// red-canary drift cannot be retried — nor its operator notice re-sent — every
/// tick. Concurrency-safe via a CAS loop (the daemon runs one tick at a time, but
/// this stays correct if that ever changes).
pub fn global_deploy_throttle_allow(now_secs: u64, min_interval_secs: u64) -> bool {
    use std::sync::atomic::Ordering;
    let floor = min_interval_secs.max(MIN_DEPLOY_INTERVAL_FLOOR);
    loop {
        let prev = LAST_DEPLOY_ATTEMPT_SECS.load(Ordering::Acquire);
        if prev != 0 && now_secs.saturating_sub(prev) < floor {
            return false;
        }
        if LAST_DEPLOY_ATTEMPT_SECS
            .compare_exchange(prev, now_secs, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }
    }
}

/// Reset the process-global deploy-attempt clock. TEST-ONLY: the throttle is a
/// static shared across the whole process, so a test that exercises it must
/// clear it to stay independent of ordering.
#[cfg(test)]
pub(crate) fn reset_global_deploy_throttle() {
    LAST_DEPLOY_ATTEMPT_SECS.store(0, std::sync::atomic::Ordering::SeqCst);
}

/// Serialises every test that touches the process-global deploy throttle (in this
/// module AND the overseer OBSERVE-rail tests), since `cargo test` runs tests in
/// parallel threads that share the one static. Poisoning is tolerated — the guard
/// only orders access, it protects no invariant of its own.
#[cfg(test)]
pub(crate) static DEPLOY_THROTTLE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire [`DEPLOY_THROTTLE_TEST_LOCK`], tolerating poisoning from a panicking
/// sibling test.
#[cfg(test)]
pub(crate) fn deploy_throttle_test_guard() -> std::sync::MutexGuard<'static, ()> {
    DEPLOY_THROTTLE_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Observe deploy drift for the acting Overseer's OBSERVE stage (issue #2590).
///
/// Fail-SAFE by contract: an implementation maps ANY git/source error — a
/// missing checkout, a failed fetch, an unresolved head — to `None`, so an
/// error never triggers a deploy. `Some(observation)` is returned ONLY when the
/// running binary is genuinely behind merged `main` AND a non-empty target
/// commit resolved.
pub trait DeployDriftObserver: Send + Sync {
    /// The fail-safe drift observation, or `None` when current / unresolved /
    /// on any error.
    fn observe(&self) -> Option<DeployDriftObservation>;
}

/// Production [`DeployDriftObserver`]: resolves the daemon's own source checkout
/// cwd-independently (`SIMARD_SELF_DEPLOY_REPO` → persistent `~/.simard`
/// checkout), best-effort fetches `origin`, then runs the fail-safe
/// [`ReconcileDetector`] over a [`GitDeploySource`]. Mirrors the read-only
/// resolution the operator `simard self-deploy --check` uses, so the drift the
/// daemon acts on is the SAME drift an operator would see.
#[derive(Default)]
pub struct GitDeployDriftObserver;

impl GitDeployDriftObserver {
    pub fn new() -> Self {
        Self
    }
}

impl DeployDriftObserver for GitDeployDriftObserver {
    fn observe(&self) -> Option<DeployDriftObservation> {
        // Resolve the canonical source checkout cwd-independently. When none
        // exists yet (e.g. before the first deploy) fall back to the cwd — the
        // detector is fail-safe either way.
        let preparer = GitSourcePreparer::new();
        let source = match preparer.resolve_existing_repo() {
            Some(repo) => {
                // Best-effort refresh so the merged head is current; a failed
                // fetch (e.g. offline) degrades to the local tracking refs
                // rather than aborting — the detector still fails safe.
                let _ = preparer.fetch_origin(&repo);
                GitDeploySource::at(repo)
            }
            None => GitDeploySource::new(),
        };
        // Fail-safe: `detect` maps any source error to `needs_deploy: false`.
        let drift = ReconcileDetector::new(source.clone()).detect();
        if !drift.needs_deploy {
            return None;
        }
        // Never deploy blind: require a resolved, non-empty merged head.
        let target = source.merged_head().ok()?;
        let target = target.trim();
        if target.is_empty() {
            return None;
        }
        Some(DeployDriftObservation {
            target_commit: target.to_string(),
            behind_commits: drift.behind_commits,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_signal_present_when_behind_and_target_resolved() {
        let drift = DeployDrift::from_parts(3, Vec::new());
        let sig = deploy_drift_signal(&drift, "deadbeefcafe").expect("signal");
        assert_eq!(
            sig,
            Signal::DeployDriftDetected {
                target_commit: "deadbeefcafe".to_string(),
                behind_commits: 3,
            }
        );
    }

    #[test]
    fn drift_signal_absent_when_current() {
        // `DeployDrift::current()` is the fail-safe shape a git error also yields.
        assert!(deploy_drift_signal(&DeployDrift::current(), "deadbeef").is_none());
    }

    #[test]
    fn drift_signal_absent_when_target_commit_empty() {
        // needs_deploy but no resolvable head → no signal (fail-closed: never a
        // deploy with an empty target).
        let drift = DeployDrift::from_parts(2, Vec::new());
        assert!(deploy_drift_signal(&drift, "   ").is_none());
    }

    #[test]
    fn drift_signal_present_for_pin_drift_only() {
        // Pin drift (no behind_commits) still needs a deploy.
        let drift = DeployDrift::from_parts(0, vec!["amplihack-memory".to_string()]);
        assert!(drift.needs_deploy);
        let sig = deploy_drift_signal(&drift, "abc123").expect("signal");
        assert_eq!(
            sig,
            Signal::DeployDriftDetected {
                target_commit: "abc123".to_string(),
                behind_commits: 0,
            }
        );
    }

    #[test]
    fn global_throttle_blocks_second_attempt_within_window() {
        // The process-global guard is what actually throttles production (the
        // per-tick-rebuilt Overseer cannot hold instance state).
        let _guard = deploy_throttle_test_guard();
        reset_global_deploy_throttle();
        assert!(
            global_deploy_throttle_allow(10_000, 900),
            "first attempt allowed"
        );
        assert!(
            !global_deploy_throttle_allow(10_300, 900),
            "second attempt within the 15-min window is throttled"
        );
        assert!(
            global_deploy_throttle_allow(10_900, 900),
            "attempt after the window is allowed"
        );
        reset_global_deploy_throttle();
    }

    #[test]
    fn global_throttle_floor_applies_to_a_mis_set_interval() {
        let _guard = deploy_throttle_test_guard();
        reset_global_deploy_throttle();
        // Interval below the floor is clamped up to the floor.
        assert!(global_deploy_throttle_allow(1, 1));
        assert!(
            !global_deploy_throttle_allow(1 + MIN_DEPLOY_INTERVAL_FLOOR - 1, 1),
            "within the floored window is blocked"
        );
        reset_global_deploy_throttle();
    }

    #[test]
    fn autonomous_deploy_enabled_default_and_optout() {
        // Serialise env mutation within this test only; default is enabled.
        let prev = std::env::var(AUTONOMOUS_DEPLOY_ENV).ok();
        // SAFETY: single-threaded test-local env toggle, restored below.
        unsafe {
            std::env::remove_var(AUTONOMOUS_DEPLOY_ENV);
        }
        assert!(autonomous_deploy_enabled(), "enabled by default");
        for off in ["0", "false", "OFF", "No"] {
            unsafe {
                std::env::set_var(AUTONOMOUS_DEPLOY_ENV, off);
            }
            assert!(!autonomous_deploy_enabled(), "opt-out via {off:?}");
        }
        unsafe {
            std::env::set_var(AUTONOMOUS_DEPLOY_ENV, "1");
        }
        assert!(autonomous_deploy_enabled(), "any other value stays enabled");
        // Restore.
        unsafe {
            match prev {
                Some(v) => std::env::set_var(AUTONOMOUS_DEPLOY_ENV, v),
                None => std::env::remove_var(AUTONOMOUS_DEPLOY_ENV),
            }
        }
    }
}
