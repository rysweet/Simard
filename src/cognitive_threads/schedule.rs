//! Pure, injected-clock scheduling math (Appendix A.4).
//!
//! Every function here is a pure function of its arguments (no `SystemTime`,
//! no sleeps) so due-computation and backoff are fully unit-testable. The tests
//! in `super::tests` pin the contract.

use std::time::Duration;

use super::thread::SchedulePolicy;

/// Minimum cadence floor for interval policies (SR-8). Any configured interval
/// at or below this is clamped up to it so a hostile/misconfigured `0` cannot
/// make a thread due every tick.
pub const MIN_INTERVAL_SECS: u64 = 60;

/// Upper bound on the backoff doubling shift so `1 << shift` can never overflow
/// `u64` regardless of the (untrusted) error count.
const BACKOFF_MAX_SHIFT: u32 = 32;

/// The fixed cadence (seconds) of a policy, or `None` for the trigger-based
/// policies that have no interval.
fn interval_secs(policy: &SchedulePolicy) -> Option<u64> {
    match policy {
        SchedulePolicy::Interval(d) => Some(d.as_secs()),
        SchedulePolicy::Adaptive { current, .. } => Some(current.as_secs()),
        SchedulePolicy::OnDemand | SchedulePolicy::EventDriven => None,
    }
}

/// Whether `policy` is due at `now_epoch` given the last run.
///
/// Contract:
/// - `Interval(d)` / `Adaptive{current,..}`: due when `last_run` is `None`
///   (never run) or `now >= last_run + interval`.
/// - `OnDemand` / `EventDriven`: never auto-due from this pure function (their
///   trigger is an explicit request/predicate handled by the caller).
pub fn is_due(policy: &SchedulePolicy, last_run_epoch: Option<u64>, now_epoch: u64) -> bool {
    match interval_secs(policy) {
        None => false,
        Some(interval) => match last_run_epoch {
            None => true,
            Some(last) => now_epoch >= last.saturating_add(interval),
        },
    }
}

/// The next scheduled run epoch, or `None` for policies with no fixed cadence
/// (`OnDemand`/`EventDriven`). For `Interval`/`Adaptive` returns
/// `last_run + interval` (or `now` when never run).
pub fn next_run_epoch(
    policy: &SchedulePolicy,
    last_run_epoch: Option<u64>,
    now_epoch: u64,
) -> Option<u64> {
    interval_secs(policy).map(|interval| match last_run_epoch {
        None => now_epoch,
        Some(last) => last.saturating_add(interval),
    })
}

/// Capped exponential backoff: `now + min(base * 2^min(errors, shift_cap), cap)`
/// in seconds, saturating (never overflows, never exceeds `now + cap`).
pub fn backoff_until_epoch(
    now_epoch: u64,
    consecutive_errors: u32,
    base: Duration,
    cap: Duration,
) -> u64 {
    let base_secs = base.as_secs();
    let cap_secs = cap.as_secs();
    let shift = consecutive_errors.min(BACKOFF_MAX_SHIFT);
    // `1 << shift` fits in u64 for shift <= 32; multiplication saturates so a
    // large base or error count can never overflow.
    let factor = 1u64 << shift;
    let delay = base_secs.saturating_mul(factor).min(cap_secs);
    now_epoch.saturating_add(delay)
}

/// Clamp a configured interval (seconds) up to [`MIN_INTERVAL_SECS`] (SR-8).
/// Values already above the floor are returned unchanged.
pub fn clamp_interval_secs(raw: u64) -> u64 {
    raw.max(MIN_INTERVAL_SECS)
}

/// Env var applying a single global multiplier to every cognitive-thread
/// interval — the cheap "slow all ten at once" cost-control knob.
pub const INTERVAL_SCALE_ENV: &str = "SIMARD_THREAD_INTERVAL_SCALE";

/// Apply the global [`INTERVAL_SCALE_ENV`] multiplier (float, default `1.0`;
/// non-finite / non-positive / unparseable values are ignored) to `raw`, then
/// clamp to [`MIN_INTERVAL_SECS`]. Applied once in each thread's `from_env` so
/// the floor still holds after scaling.
pub fn scale_and_clamp_interval_secs(raw: u64) -> u64 {
    let scale = std::env::var(INTERVAL_SCALE_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0);
    let scaled = (raw as f64 * scale).round();
    let scaled = if scaled.is_finite() && scaled >= 0.0 {
        scaled as u64
    } else {
        raw
    };
    clamp_interval_secs(scaled)
}
