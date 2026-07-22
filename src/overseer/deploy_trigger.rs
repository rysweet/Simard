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
    lock_poisoned(&DEPLOY_THROTTLE_TEST_LOCK)
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
    // One uniform pattern per byte (lowercase hex only — deliberately NOT
    // `is_ascii_hexdigit`, which also accepts `A`–`F`). A `matches!` range
    // pattern lowers to direct bounded comparisons, avoiding the extra work
    // `RangeInclusive::contains` does per element.
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// ═══════════════════════════ red-canary loop-halt ═══════════════════════════
//
// Seam B (issue #4420): the self-deploy loop was crash-looping for 8h+ — every
// tick re-attempted the SAME failing SHA, refused on a red canary, and silently
// let DeployDrift grow (1 → 6 commits behind main). The anti-thrash throttle
// above only spaces attempts in TIME; it never STOPS a persistently-red SHA. So
// this counts CONSECUTIVE red canaries per target SHA and, past a threshold,
// lets the OBSERVE rail HALT re-signalling that SHA and escalate ONCE to the
// operator instead of looping blind.
//
// Composes with (does NOT duplicate) the anti-thrash throttle: the throttle
// bounds attempt frequency; this bounds attempt COUNT for a stuck SHA.

/// Env var overriding the consecutive-red-canary count at which a stuck SHA
/// halts and escalates. In the `SIMARD_OVERSEER_DEPLOY_*` namespace. A missing
/// or unparseable value uses [`DEFAULT_RED_CANARY_HALT`]; any value below
/// [`RED_CANARY_HALT_FLOOR`] is clamped up so the guard can never be disabled.
pub const RED_CANARY_HALT_ENV: &str = "SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT";

/// Default consecutive red canaries before a SHA is treated as stuck. Three
/// gives a genuinely-flaky gate two retries before escalating, while still
/// catching a hard regression well before drift grows unbounded.
pub const DEFAULT_RED_CANARY_HALT: u32 = 3;

/// Hard floor for the halt threshold: a mis-set env can never disable the guard
/// (availability hardening — a silently-looping self-deploy is the exact fault
/// #4420 fixes).
pub const RED_CANARY_HALT_FLOOR: u32 = 2;

/// PROCESS-GLOBAL consecutive-red-canary streak for the single SHA currently
/// stuck. `None` when no SHA is stuck; `Some((sha, count))` otherwise. Holds AT
/// MOST ONE active entry (bounded memory): a red canary for a NEW target
/// supersedes the prior one, since a fresh merged head is a fresh attempt. Must
/// be a `static` for the same reason the throttle is — the daemon rebuilds the
/// acting `Overseer` every tick, so per-instance state could never accumulate a
/// streak across ticks.
static RED_CANARY_STREAK: std::sync::Mutex<Option<(String, u32)>> = std::sync::Mutex::new(None);

/// PROCESS-GLOBAL one-shot latch for the stuck-escalation: the SHA already
/// escalated to the operator, so a persisting stuck loop escalates AT MOST ONCE
/// (no alert flooding) even though the OBSERVE rail runs every tick. At most one
/// entry: a new stuck SHA replaces it.
static DEPLOY_STUCK_ESCALATED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Acquire a process-global mutex, tolerating poisoning from a panicking
/// sibling (these guards only ORDER access — they protect no invariant of their
/// own, so an inner value left by a panicked holder is safe to reuse).
fn lock_poisoned<T>(m: &'static std::sync::Mutex<T>) -> std::sync::MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_streak() -> std::sync::MutexGuard<'static, Option<(String, u32)>> {
    lock_poisoned(&RED_CANARY_STREAK)
}

fn lock_escalated() -> std::sync::MutexGuard<'static, Option<String>> {
    lock_poisoned(&DEPLOY_STUCK_ESCALATED)
}

/// Record one canary OUTCOME for `sha` at the ACT/canary site. `is_red == true`
/// (the canary reddened) increments that SHA's consecutive streak; `false` (the
/// canary went green) CLEARS it — the SHA is no longer stuck, so the loop-halt
/// guard re-arms from zero AND the one-shot stuck-escalation latch re-arms, so a
/// fresh stall on the same SHA can escalate to the operator again (this matches
/// the documented "re-arms when the canary goes green" contract). A red for a
/// NEW target starts its own count and clears any superseded SHA (at most one
/// active stuck entry).
pub(crate) fn record_red_canary_result(sha: &str, is_red: bool) {
    if is_red {
        let mut streak = lock_streak();
        match streak.as_mut() {
            Some((s, count)) if s == sha => *count = count.saturating_add(1),
            _ => *streak = Some((sha.to_string(), 1)),
        }
        return;
    }
    // Green result: the SHA is no longer stuck. Clear its streak, then re-arm the
    // one-shot escalation latch so a subsequent stall re-escalates. The two
    // process-global locks are taken in the streak→escalated order used
    // everywhere else (never held simultaneously) to keep the ordering total.
    {
        let mut streak = lock_streak();
        if matches!(streak.as_ref(), Some((s, _)) if s == sha) {
            *streak = None;
        }
    }
    {
        let mut latch = lock_escalated();
        if matches!(latch.as_ref(), Some(s) if s == sha) {
            *latch = None;
        }
    }
}

/// The current consecutive red-canary streak for `sha` (`0` when it is not the
/// stuck SHA). READ by the OBSERVE rail to decide whether to halt.
pub(crate) fn red_canary_streak_for(sha: &str) -> u32 {
    match lock_streak().as_ref() {
        Some((s, count)) if s == sha => *count,
        _ => 0,
    }
}

/// The consecutive-red-canary count at which a SHA halts, from
/// [`RED_CANARY_HALT_ENV`] (default [`DEFAULT_RED_CANARY_HALT`]), clamped to at
/// least [`RED_CANARY_HALT_FLOOR`] so the guard is never disabled.
pub(crate) fn red_canary_halt_threshold() -> u32 {
    std::env::var(RED_CANARY_HALT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_RED_CANARY_HALT)
        .max(RED_CANARY_HALT_FLOOR)
}

/// Latch `sha` as escalated, returning `true` only the FIRST time (so the caller
/// escalates once) and `false` on every subsequent tick for that same SHA. A new
/// stuck SHA replaces the latched one.
pub(crate) fn mark_deploy_stuck_escalated(sha: &str) -> bool {
    let mut latch = lock_escalated();
    if matches!(latch.as_ref(), Some(s) if s == sha) {
        return false;
    }
    *latch = Some(sha.to_string());
    true
}

/// Reset the process-global red-canary streak AND the stuck-escalation latch.
/// TEST-ONLY: both are statics shared across the whole process, so a test that
/// exercises them must clear them to stay independent of ordering.
#[cfg(test)]
pub(crate) fn reset_red_canary_streak() {
    *lock_streak() = None;
    *lock_escalated() = None;
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

    // ════════════════════════════════════════════════════════════════════════
    // Seam B (issue #4420): consecutive red-canary streak — the loop-halt STATE.
    //
    // TDD (RED): these reference `record_red_canary_result`,
    // `red_canary_streak_for`, `red_canary_halt_threshold`,
    // `reset_red_canary_streak`, `RED_CANARY_HALT_ENV`, and
    // `DEFAULT_RED_CANARY_HALT`, none of which exist yet — the crate test build
    // FAILS to compile until Seam B lands. The streak is a process-global
    // (per-SHA) counter written at the ACT/canary site and READ by the OBSERVE
    // rail; these unit tests pin the pure state machine (no Overseer wiring).
    //
    // NOTE (no-collision contract with sibling PR #4409): Seam B introduces NO
    // `deploy_throttle.rs` and NO `DeployAttemptLedger` — it is an orthogonal,
    // in-process escalation counter that composes with #4409's durable ledger.
    // ════════════════════════════════════════════════════════════════════════

    /// T2 — each consecutive red canary for the same SHA increments its streak.
    #[test]
    fn red_canary_streak_increments_on_consecutive_reds() {
        let _guard = deploy_throttle_test_guard();
        reset_red_canary_streak();
        let sha = "a".repeat(40);
        assert_eq!(red_canary_streak_for(&sha), 0, "no reds recorded yet");
        record_red_canary_result(&sha, true);
        assert_eq!(red_canary_streak_for(&sha), 1);
        record_red_canary_result(&sha, true);
        record_red_canary_result(&sha, true);
        assert_eq!(
            red_canary_streak_for(&sha),
            3,
            "each consecutive red canary increments the streak"
        );
        reset_red_canary_streak();
    }

    /// T3 — a GREEN (passing) canary result clears the streak: the SHA is no
    /// longer stuck, so the loop-halt guard must re-arm from zero.
    #[test]
    fn red_canary_streak_resets_on_green() {
        let _guard = deploy_throttle_test_guard();
        reset_red_canary_streak();
        let sha = "b".repeat(40);
        record_red_canary_result(&sha, true);
        record_red_canary_result(&sha, true);
        assert_eq!(red_canary_streak_for(&sha), 2);
        record_red_canary_result(&sha, false);
        assert_eq!(
            red_canary_streak_for(&sha),
            0,
            "a passing canary resets the streak for that SHA"
        );
        reset_red_canary_streak();
    }

    /// T3a — a GREEN canary re-arms the one-shot stuck-escalation latch (doc
    /// contract: "re-arms when the canary goes green"). A SHA that escalated,
    /// recovered (green), then re-stalls must be allowed to escalate ONCE more —
    /// otherwise a flapping deploy could go permanently silent after its first
    /// escalation. `mark_deploy_stuck_escalated` returns `true` only on a fresh
    /// arm, so we assert the second stall re-arms.
    #[test]
    fn green_canary_rearms_the_stuck_escalation_latch() {
        let _guard = deploy_throttle_test_guard();
        reset_red_canary_streak();
        let sha = "c".repeat(40);
        // First stall escalates once; the latch is now set for this SHA.
        assert!(
            mark_deploy_stuck_escalated(&sha),
            "first stall arms and escalates"
        );
        assert!(
            !mark_deploy_stuck_escalated(&sha),
            "still latched — no re-escalation while stuck"
        );
        // Canary recovers (green): the latch must re-arm for this same SHA.
        record_red_canary_result(&sha, false);
        assert!(
            mark_deploy_stuck_escalated(&sha),
            "a green result re-arms the one-shot latch, so a fresh stall escalates again"
        );
        reset_red_canary_streak();
    }

    /// a red for a NEW target starts its own count and clears the superseded SHA
    /// (bounded memory; a new target is a fresh attempt, design steady-state).
    #[test]
    fn red_canary_streak_is_per_sha_with_one_active_entry() {
        let _guard = deploy_throttle_test_guard();
        reset_red_canary_streak();
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        record_red_canary_result(&a, true);
        record_red_canary_result(&a, true);
        assert_eq!(red_canary_streak_for(&a), 2);
        // A different target supersedes the prior one.
        record_red_canary_result(&b, true);
        assert_eq!(red_canary_streak_for(&b), 1, "a new target starts fresh");
        assert_eq!(
            red_canary_streak_for(&a),
            0,
            "the superseded SHA is cleared (at most one active stuck entry)"
        );
        reset_red_canary_streak();
    }

    /// T3c — the halt threshold: documented default when unset, garbage falls
    /// back to the default, and a sub-floor value is clamped up so the guard can
    /// never be disabled (availability hardening). Env-mutating, so serialised.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn red_canary_halt_threshold_default_garbage_and_floor() {
        let prev = std::env::var(RED_CANARY_HALT_ENV).ok();
        // SAFETY: single-threaded, test-local env toggles restored at the end.
        unsafe {
            std::env::remove_var(RED_CANARY_HALT_ENV);
        }
        assert_eq!(
            red_canary_halt_threshold(),
            DEFAULT_RED_CANARY_HALT,
            "unset ⇒ documented default"
        );
        assert_eq!(DEFAULT_RED_CANARY_HALT, 3, "documented default is 3");
        unsafe {
            std::env::set_var(RED_CANARY_HALT_ENV, "not-a-number");
        }
        assert_eq!(
            red_canary_halt_threshold(),
            DEFAULT_RED_CANARY_HALT,
            "garbage ⇒ default (defensive parse)"
        );
        for disabling in ["0", "1"] {
            unsafe {
                std::env::set_var(RED_CANARY_HALT_ENV, disabling);
            }
            assert!(
                red_canary_halt_threshold() >= 2,
                "a sub-floor value ({disabling:?}) is clamped up to keep the guard armed"
            );
        }
        unsafe {
            std::env::set_var(RED_CANARY_HALT_ENV, "5");
        }
        assert_eq!(
            red_canary_halt_threshold(),
            5,
            "a sane explicit override is honoured"
        );
        unsafe {
            match prev {
                Some(v) => std::env::set_var(RED_CANARY_HALT_ENV, v),
                None => std::env::remove_var(RED_CANARY_HALT_ENV),
            }
        }
    }

    /// T3d — the env var name matches the `SIMARD_OVERSEER_DEPLOY_*` sibling
    /// namespace (documentation/consistency contract from the doc review).
    #[test]
    fn red_canary_halt_env_var_is_in_the_overseer_deploy_namespace() {
        assert_eq!(
            RED_CANARY_HALT_ENV,
            "SIMARD_OVERSEER_DEPLOY_RED_CANARY_HALT"
        );
    }
}
