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

// ───────────── class-aware bounded transient-red backoff (Brick C, #4415) ────
//
// The process-global anti-thrash throttle above already stops the daemon
// redeploying every tick. Brick C layers a class-aware bounded EXPONENTIAL
// backoff on top, keyed on the last canary's `transient` class, so a FLAKY red
// canary is retried on a widening (but capped) interval instead of hammering
// the same failing build every base interval — while a DETERMINISTIC red canary
// is simply held at the base interval (waiting will not fix a real regression).
//
// State is in-memory (process-global, like the throttle) and idempotent: a green
// canary resets it, a deterministic red clears it, and restarting the daemon
// starts from a clean slate. There is NO persisted poison state.

/// Env var for the transient-backoff BASE interval (seconds). Reuses the
/// existing anti-thrash min-interval knob so the backoff base is the same
/// base interval an operator already tunes (see the reference doc).
pub const TRANSIENT_BACKOFF_BASE_ENV: &str = DEPLOY_MIN_INTERVAL_ENV;

/// Default transient-backoff base: the same 15-minute anti-thrash default.
pub const DEFAULT_TRANSIENT_BACKOFF_BASE_SECS: u64 = DEFAULT_DEPLOY_INTERVAL_SECS;

/// Non-zero floor for the backoff base so a mis-set base can never collapse the
/// transient retry into a zero-delay busy-loop (self-DoS).
pub const MIN_TRANSIENT_BACKOFF_BASE_SECS: u64 = 1;

/// Absolute ceiling for the backed-off transient interval (2 hours). A
/// pathological streak saturates here rather than growing to an unbounded sleep.
pub const TRANSIENT_BACKOFF_CEILING_SECS: u64 = 7200;

/// Resolve the transient-backoff base interval (seconds) from
/// [`TRANSIENT_BACKOFF_BASE_ENV`]. Fail-safe: unset/empty/unparseable falls back
/// to [`DEFAULT_TRANSIENT_BACKOFF_BASE_SECS`]; the result is floored to
/// [`MIN_TRANSIENT_BACKOFF_BASE_SECS`]. Never panics.
pub fn transient_backoff_base_secs() -> u64 {
    std::env::var(TRANSIENT_BACKOFF_BASE_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_TRANSIENT_BACKOFF_BASE_SECS)
        .max(MIN_TRANSIENT_BACKOFF_BASE_SECS)
}

/// Pure class-aware backoff: the interval (seconds) before the next retry after
/// `consecutive` consecutive TRANSIENT red canaries at `base` seconds. Zero
/// consecutive failures ⇒ `0` (nothing to recover from). Otherwise
/// `min(base * 2^(n-1), TRANSIENT_BACKOFF_CEILING_SECS)` with `base` floored to
/// [`MIN_TRANSIENT_BACKOFF_BASE_SECS`]. All arithmetic is SATURATING: a
/// pathological streak clamps to the ceiling and never overflows/panics.
pub fn transient_backoff_secs(consecutive: u32, base: u64) -> u64 {
    if consecutive == 0 {
        return 0;
    }
    let base = base.max(MIN_TRANSIENT_BACKOFF_BASE_SECS);
    let shift = consecutive - 1;
    let grown = if shift >= 63 {
        u64::MAX
    } else {
        base.saturating_mul(1u64 << shift)
    };
    grown.min(TRANSIENT_BACKOFF_CEILING_SECS)
}

/// In-memory, idempotent class-aware backoff state (Brick C, #4415). Tracks the
/// consecutive-transient-red streak and turns it into a bounded exponential
/// retry interval. A green canary or a deterministic red RESETS the streak, so a
/// single flaky red can never permanently widen the interval and self-deploy
/// converges the instant the canary goes green. No persisted state.
#[derive(Clone, Debug)]
pub struct TransientRedBackoff {
    consecutive_transient: u32,
    base_secs: u64,
}

impl TransientRedBackoff {
    /// New backoff seeded with the env-resolved base interval.
    pub fn new() -> Self {
        Self {
            consecutive_transient: 0,
            base_secs: transient_backoff_base_secs(),
        }
    }

    /// Record a TRANSIENT red canary: grow the streak and return the new
    /// (bounded) delay before the next retry.
    pub fn record_transient_red(&mut self) -> u64 {
        self.consecutive_transient = self.consecutive_transient.saturating_add(1);
        self.current_delay_secs()
    }

    /// Record a DETERMINISTIC red canary: it does not fast-retry, so clear the
    /// transient streak (the existing base-interval throttle owns its cadence).
    pub fn record_deterministic_red(&mut self) {
        self.consecutive_transient = 0;
    }

    /// Record a GREEN canary / successful deploy: reset the streak so the next
    /// transient failure starts backoff from the base again (convergence).
    pub fn record_green(&mut self) {
        self.consecutive_transient = 0;
    }

    /// The current bounded delay (seconds) for the recorded streak. `0` when the
    /// streak is clear.
    pub fn current_delay_secs(&self) -> u64 {
        transient_backoff_secs(self.consecutive_transient, self.base_secs)
    }
}

impl Default for TransientRedBackoff {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-global consecutive-transient-red streak driving the live backoff.
/// PROCESS-GLOBAL for the same reason as [`LAST_DEPLOY_ATTEMPT_SECS`]: the daemon
/// rebuilds the acting `Overseer` every tick, so per-instance state could never
/// persist a streak across ticks. `0` means "no transient streak".
static CONSECUTIVE_TRANSIENT_REDS: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

/// Record the class of the canary from a deploy ATTEMPT into the process-global
/// backoff streak. A green canary or a deterministic red RESETS the streak; a
/// transient red GROWS it (saturating). Idempotent per class. Emits a structured
/// tracing event (OTel attributes `consecutive_transient` / `backoff_secs`) on a
/// transient red so the widening retry cadence is diagnosable — no `println!`.
pub fn record_canary_outcome(passed: bool, transient: bool) {
    use std::sync::atomic::Ordering;
    if passed || !transient {
        // Green canary, or a deterministic red: converge / hold at base.
        CONSECUTIVE_TRANSIENT_REDS.store(0, Ordering::Release);
        return;
    }
    // Transient red: grow the bounded streak.
    let prev = CONSECUTIVE_TRANSIENT_REDS.load(Ordering::Acquire);
    let next = prev.saturating_add(1);
    CONSECUTIVE_TRANSIENT_REDS.store(next, Ordering::Release);
    // Log the SAME base the OBSERVE rail actually enforces
    // (`deploy_min_interval_secs`, floored at `MIN_DEPLOY_INTERVAL_FLOOR`), not
    // the more permissive `transient_backoff_base_secs` floor, so the diagnosed
    // `backoff_secs` never understates the interval `effective_deploy_min_interval_secs`
    // will impose.
    let backoff_secs = transient_backoff_secs(next, deploy_min_interval_secs());
    tracing::warn!(
        target: "overseer::deploy_trigger",
        consecutive_transient = next,
        backoff_secs,
        "transient red canary: backing off before the next self-deploy retry"
    );
}

/// The EFFECTIVE anti-thrash minimum interval for the next deploy attempt: the
/// base interval widened by the class-aware transient backoff when a transient
/// red streak is in effect, else the plain base interval. Never below the base.
pub fn effective_deploy_min_interval_secs() -> u64 {
    use std::sync::atomic::Ordering;
    let base = deploy_min_interval_secs();
    let streak = CONSECUTIVE_TRANSIENT_REDS.load(Ordering::Acquire);
    if streak == 0 {
        return base;
    }
    transient_backoff_secs(streak, base).max(base)
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
        //
        // `origin_strict` (issue #2590 SR-1): on this autonomous path the source
        // NEVER falls back to local `HEAD`. An unresolved `origin/<default>` is
        // treated as an error → `needs_deploy: false` → no drift → no signal,
        // so the daemon can never autonomously deploy an unverified local `HEAD`
        // that skipped branch protection / signed-merge (the sole documented
        // root of trust). The operator/CLI path keeps the `HEAD` fallback.
        let preparer = GitSourcePreparer::new();
        let source = match preparer.resolve_existing_repo() {
            Some(repo) => {
                // Best-effort refresh so the merged head is current; a failed
                // fetch (e.g. offline) degrades to the local tracking refs
                // rather than aborting — the detector still fails safe.
                let _ = preparer.fetch_origin(&repo);
                GitDeploySource::at(repo).origin_strict()
            }
            None => GitDeploySource::new().origin_strict(),
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
        // Defense-in-depth (issue #2590 SR-2): the documented trust model
        // validates `target_commit` as a full 40/64-char lowercase hex SHA
        // before it is ever handed to the build/checkout. Enforce it here, at
        // the boundary, so a non-SHA target can never reach `run_canary` or the
        // orchestrator checkout — an unexpected shape yields no signal.
        if !is_full_hex_sha(target) {
            return None;
        }
        Some(DeployDriftObservation {
            target_commit: target.to_string(),
            behind_commits: drift.behind_commits,
        })
    }
}

/// Is `s` a full git object name — a 40-char (SHA-1) or 64-char (SHA-256)
/// lowercase hex SHA? The documented trust-model shape for an autonomous deploy
/// target (issue #2590 SR-2); stricter than `is_hex_commitish` (4–64, any case)
/// so only a fully-resolved commit id — never an abbreviation or ref — is
/// accepted on the unattended path.
fn is_full_hex_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64)
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
    fn full_hex_sha_accepts_40_and_64_lowercase_only() {
        // SR-2 (#2590): only a full 40 (SHA-1) or 64 (SHA-256) lowercase hex id
        // is accepted as an autonomous deploy target.
        assert!(is_full_hex_sha(&"a".repeat(40)));
        assert!(is_full_hex_sha(&"0".repeat(64)));
        assert!(is_full_hex_sha("da39a3ee5e6b4b0d3255bfef95601890afd80709"));
        // Wrong length (abbreviations / short hashes the gate must reject).
        assert!(!is_full_hex_sha("deadbeef"));
        assert!(!is_full_hex_sha(&"a".repeat(39)));
        assert!(!is_full_hex_sha(&"a".repeat(41)));
        assert!(!is_full_hex_sha(&"a".repeat(63)));
        // Non-hex / uppercase / refs.
        assert!(!is_full_hex_sha(&"A".repeat(40)));
        assert!(!is_full_hex_sha("HEAD"));
        assert!(!is_full_hex_sha(&format!("{}z", "a".repeat(39))));
        assert!(!is_full_hex_sha(""));
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
    #[serial_test::serial(cognitive_memory)]
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
