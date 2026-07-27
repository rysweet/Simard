//! Reclaim effectiveness gate — the suppress-only cooldown that stops the OODA
//! daemon re-running agentic disk-reclamation every cycle when reclamation keeps
//! freeing nothing (issues #4809 / #4825 / #4810).
//!
//! # Why this exists
//! The plain `%-used` trigger re-fired a *proven-ineffective* reclaim run every
//! ~15 minutes on the ~94%-full host — churn that burned CPU/IO and log volume
//! without freeing a byte. This gate wraps the same bounded-exponential-backoff
//! semantics as [`crate::overseer::guardrails::BackoffGate`] into a **suppress
//! -only** pre-filter: it decides *whether* a run happens, and can never change
//! *how* it runs (a dry-run can never become an apply).
//!
//! # Contract (mirrors `BackoffGate`'s peek/record split)
//! - [`ReclaimEffectivenessGate::new`]`(base, multiplier, max, hard_ceiling_pct)`
//! - [`ReclaimEffectivenessGate::peek`] decides WITHOUT recording. A fresh local
//!   `used_pct` at/above `hard_ceiling_pct` always returns [`Run`] (a genuinely
//!   filling disk always reclaims). Otherwise an unseen key, an elapsed cooldown,
//!   or a backwards clock jump returns [`Run`]; a re-hit strictly inside the
//!   current cooldown (below ceiling) returns [`Suppress`].
//! - [`ReclaimEffectivenessGate::record`] records the OUTCOME of a run that
//!   actually happened: `effective == true` re-admits the next cycle (streak → 0,
//!   no cooldown armed); `effective == false` arms/grows the cooldown window
//!   `× multiplier` (saturating, capped). A silence `>= 2× window` resets to base.
//!
//! [`Run`]: EffectivenessDecision::Run
//! [`Suppress`]: EffectivenessDecision::Suppress
//!
//! See `docs/reference/reclaim-effectiveness-gate-api.md` for the full contract
//! and `docs/concepts/reclaim-effectiveness-backoff.md` for the rationale.

use std::collections::HashMap;

/// Master kill switch (see the operations doc). `off`/`false`/`0`/`no` (case
/// -insensitive) reverts to the fire-every-over-threshold-cycle behaviour.
pub const EFFECTIVENESS_GATE_ENV: &str = "SIMARD_DISK_RECLAIM_EFFECTIVENESS_GATE";
/// Base cooldown window (seconds) after the first no-op run.
pub const COOLDOWN_BASE_SECS_ENV: &str = "SIMARD_DISK_RECLAIM_COOLDOWN_BASE_SECS";
/// Growth factor per additional no-op run (`>= 2`; lower values clamp to `2`).
pub const COOLDOWN_MULTIPLIER_ENV: &str = "SIMARD_DISK_RECLAIM_COOLDOWN_MULTIPLIER";
/// Hard cap (seconds) on the cooldown window.
pub const COOLDOWN_MAX_SECS_ENV: &str = "SIMARD_DISK_RECLAIM_COOLDOWN_MAX_SECS";
/// Locally-observed `%-used` at/above which suppression is bypassed.
pub const HARD_CEILING_PCT_ENV: &str = "SIMARD_DISK_RECLAIM_HARD_CEILING_PCT";

/// Default base cooldown window: 15 minutes.
pub const DEFAULT_COOLDOWN_BASE_SECS: i64 = 900;
/// Default cooldown growth multiplier.
pub const DEFAULT_COOLDOWN_MULTIPLIER: i64 = 2;
/// Default cooldown cap: 4 hours.
pub const DEFAULT_COOLDOWN_MAX_SECS: i64 = 14_400;
/// Default hard `%-used` ceiling above which suppression is always bypassed.
pub const DEFAULT_HARD_CEILING_PCT: u8 = 97;

/// The gate's verdict for the current daemon cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectivenessDecision {
    /// Run reclamation this cycle (unseen key, cooldown elapsed, effective last
    /// time, or hard-ceiling bypass).
    Run,
    /// Skip reclamation this cycle — a streak of no-op runs is in cooldown.
    Suppress,
}

/// Per-key cooldown bookkeeping: when the last run was recorded, the current
/// suppression window, and the consecutive no-op streak (for telemetry).
#[derive(Debug, Clone, Copy)]
struct GateState {
    last_record_secs: i64,
    window_secs: i64,
    noop_streak: u32,
}

/// A suppress-only, per-key exponential-backoff cooldown on an INJECTED
/// `now_secs` clock. It **cannot alter a run's destructive posture** — it only
/// decides whether a run happens, never how. Every key backs off independently,
/// so two partitions never starve each other. On any ambiguity (clock
/// regression, overflow) it fails toward [`EffectivenessDecision::Run`] — it
/// never permanently silences a genuinely filling disk.
#[derive(Debug, Clone)]
pub struct ReclaimEffectivenessGate {
    base_window_secs: i64,
    multiplier: i64,
    max_window_secs: i64,
    hard_ceiling_pct: u8,
    state: HashMap<String, GateState>,
}

impl ReclaimEffectivenessGate {
    /// A gate whose cooldown starts at `base_window_secs` and grows
    /// `× multiplier` per consecutive no-op run, hard-capped at
    /// `max_window_secs`. `hard_ceiling_pct` is the locally-observed `%-used`
    /// at/above which suppression is always bypassed. The multiplier is floored
    /// at `2` so the window always actually grows.
    pub fn new(
        base_window_secs: i64,
        multiplier: i64,
        max_window_secs: i64,
        hard_ceiling_pct: u8,
    ) -> Self {
        Self {
            base_window_secs: base_window_secs.max(1),
            multiplier: multiplier.max(2),
            max_window_secs: max_window_secs.max(base_window_secs.max(1)),
            hard_ceiling_pct,
            state: HashMap::new(),
        }
    }

    /// Construct from the process environment (production entry point).
    pub fn from_env() -> Self {
        // Non-capturing ⇒ `Copy`, so it can be passed by value to each accessor.
        let lookup = |k: &str| std::env::var(k).ok();
        Self::new(
            cooldown_base_secs_from(lookup),
            cooldown_multiplier_from(lookup),
            cooldown_max_secs_from(lookup),
            hard_ceiling_pct_from(lookup),
        )
    }

    /// Decide WITHOUT recording — the daemon peeks, and only records the outcome
    /// *after* a run actually completes, so a suppressed cycle never advances the
    /// streak. `used_pct` is a FRESH local `df` sample (never telemetry):
    ///
    /// - `used_pct >= hard_ceiling_pct` ⇒ [`Run`] (bypass; genuine fill always
    ///   reclaims, even deep inside a cooldown),
    /// - unseen key, elapsed cooldown, or backwards clock ⇒ [`Run`],
    /// - a re-hit strictly inside the current cooldown, below ceiling ⇒
    ///   [`Suppress`].
    ///
    /// [`Run`]: EffectivenessDecision::Run
    /// [`Suppress`]: EffectivenessDecision::Suppress
    pub fn peek(&self, key: &str, used_pct: u8, now_secs: i64) -> EffectivenessDecision {
        // Ceiling bypass: a genuinely filling disk is never suppressed.
        if used_pct >= self.hard_ceiling_pct {
            return EffectivenessDecision::Run;
        }
        match self.state.get(key) {
            None => EffectivenessDecision::Run,
            Some(s) => {
                let elapsed = now_secs - s.last_record_secs;
                // Clock regression OR elapsed window ⇒ fail toward running.
                if elapsed < 0 || elapsed >= s.window_secs {
                    EffectivenessDecision::Run
                } else {
                    EffectivenessDecision::Suppress
                }
            }
        }
    }

    /// Record the OUTCOME of a run that actually happened.
    ///
    /// - `effective == true` (bytes freed / `used_pct` dropped) clears the key so
    ///   the **next cycle re-admits immediately** — an effective run is never
    ///   penalised with a cooldown.
    /// - `effective == false` (first) arms the base cooldown and streak `1`.
    /// - `effective == false` (subsequent, within `2× window`) grows the window
    ///   `× multiplier` (saturating, capped) and increments the streak.
    /// - A silence `>= 2× window` (or a backwards clock) resets to the base
    ///   window and streak `1`, so a genuinely recurring gap is never
    ///   permanently silenced.
    pub fn record(&mut self, key: &str, effective: bool, now_secs: i64) {
        if effective {
            // Effective reset: drop the key entirely so the next `peek` sees an
            // unseen key and re-admits immediately (streak → 0, no cooldown).
            self.state.remove(key);
            return;
        }
        let next = match self.state.get(key) {
            // First no-op ⇒ arm the base window.
            None => GateState {
                last_record_secs: now_secs,
                window_secs: self.base_window_secs,
                noop_streak: 1,
            },
            Some(s) => {
                let elapsed = now_secs - s.last_record_secs;
                if elapsed < 0 || elapsed >= s.window_secs.saturating_mul(2) {
                    // Long silence / clock regression ⇒ reset to base.
                    GateState {
                        last_record_secs: now_secs,
                        window_secs: self.base_window_secs,
                        noop_streak: 1,
                    }
                } else {
                    // Consecutive no-op ⇒ grow the window (saturating, capped).
                    GateState {
                        last_record_secs: now_secs,
                        window_secs: s
                            .window_secs
                            .saturating_mul(self.multiplier)
                            .min(self.max_window_secs),
                        noop_streak: s.noop_streak.saturating_add(1),
                    }
                }
            }
        };
        self.state.insert(key.to_string(), next);
    }

    /// The current consecutive no-op streak for `key` (0 if unseen / just reset).
    /// Low-cardinality telemetry only — never a raw path.
    pub fn noop_streak(&self, key: &str) -> u32 {
        self.state.get(key).map(|s| s.noop_streak).unwrap_or(0)
    }
}

// ── Configuration (injectable `lookup`, mirroring the reclaim/overseer style) ──

fn is_falsey(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

/// Whether the effectiveness gate is enabled. Default **on**; only an explicit
/// falsey value (`0`/`false`/`no`/`off`, case-insensitive) disables it —
/// unset/empty/garbage leaves it enabled.
pub fn effectiveness_gate_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    !matches!(lookup(EFFECTIVENESS_GATE_ENV).as_deref(), Some(v) if is_falsey(v))
}

/// Production entry point: read the real process environment.
pub fn effectiveness_gate_enabled() -> bool {
    effectiveness_gate_enabled_from(|k| std::env::var(k).ok())
}

/// Base cooldown seconds; invalid/non-positive → [`DEFAULT_COOLDOWN_BASE_SECS`].
pub fn cooldown_base_secs_from(lookup: impl Fn(&str) -> Option<String>) -> i64 {
    lookup(COOLDOWN_BASE_SECS_ENV)
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_COOLDOWN_BASE_SECS)
}

/// Growth multiplier; clamped to `>= 2`; invalid → [`DEFAULT_COOLDOWN_MULTIPLIER`].
pub fn cooldown_multiplier_from(lookup: impl Fn(&str) -> Option<String>) -> i64 {
    lookup(COOLDOWN_MULTIPLIER_ENV)
        .and_then(|s| s.trim().parse::<i64>().ok())
        .map(|v| v.max(2))
        .unwrap_or(DEFAULT_COOLDOWN_MULTIPLIER)
}

/// Cooldown cap seconds; invalid/non-positive → [`DEFAULT_COOLDOWN_MAX_SECS`].
pub fn cooldown_max_secs_from(lookup: impl Fn(&str) -> Option<String>) -> i64 {
    lookup(COOLDOWN_MAX_SECS_ENV)
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_COOLDOWN_MAX_SECS)
}

/// Hard `%-used` ceiling; clamped to `[1, 100]`; invalid → [`DEFAULT_HARD_CEILING_PCT`].
pub fn hard_ceiling_pct_from(lookup: impl Fn(&str) -> Option<String>) -> u8 {
    lookup(HARD_CEILING_PCT_ENV)
        .and_then(|s| s.trim().parse::<u8>().ok())
        .map(|v| v.clamp(1, 100))
        .unwrap_or(DEFAULT_HARD_CEILING_PCT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use EffectivenessDecision::{Run, Suppress};

    /// Below-ceiling `%-used`, so the ceiling bypass never masks cooldown logic.
    const LOW: u8 = 90;
    const CEILING: u8 = 97;
    const BASE: i64 = 900;

    fn gate() -> ReclaimEffectivenessGate {
        ReclaimEffectivenessGate::new(BASE, 2, 14_400, CEILING)
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    // ── peek: admission paths ────────────────────────────────────────────────

    #[test]
    fn unseen_key_runs() {
        assert_eq!(gate().peek("p", LOW, 0), Run);
    }

    #[test]
    fn ceiling_bypasses_cooldown() {
        // Arm a cooldown, then a re-hit AT the hard ceiling must still Run —
        // a genuinely filling disk always reclaims, even deep inside a cooldown.
        let mut g = gate();
        g.record("p", false, 0);
        assert_eq!(
            g.peek("p", LOW, 1),
            Suppress,
            "below ceiling, inside window"
        );
        assert_eq!(g.peek("p", CEILING, 1), Run, "at ceiling ⇒ bypass");
        assert_eq!(g.peek("p", 100, 1), Run, "above ceiling ⇒ bypass");
    }

    #[test]
    fn rehit_inside_cooldown_suppresses() {
        let mut g = gate();
        g.record("p", false, 0);
        assert_eq!(g.peek("p", LOW, BASE - 1), Suppress);
    }

    #[test]
    fn cooldown_elapsed_runs() {
        let mut g = gate();
        g.record("p", false, 0);
        assert_eq!(g.peek("p", LOW, BASE), Run, "elapsed == window admits");
        assert_eq!(g.peek("p", LOW, BASE + 10), Run);
    }

    #[test]
    fn backwards_clock_runs() {
        // Never suppress on a clock we cannot trust.
        let mut g = gate();
        g.record("p", false, 1_000);
        assert_eq!(g.peek("p", LOW, 500), Run);
    }

    // ── record: no-op backoff growth ─────────────────────────────────────────

    #[test]
    fn first_noop_arms_base_window() {
        let mut g = gate();
        g.record("p", false, 0);
        assert_eq!(g.noop_streak("p"), 1);
        assert_eq!(g.peek("p", LOW, BASE - 1), Suppress);
        assert_eq!(g.peek("p", LOW, BASE), Run);
    }

    #[test]
    fn consecutive_noops_grow_window_exponentially() {
        let mut g = gate();
        // t0: arm base (900). Re-run just as it elapses; each no-op doubles.
        g.record("p", false, 0);
        g.record("p", false, BASE); // window → 1800
        assert_eq!(g.noop_streak("p"), 2);
        assert_eq!(g.peek("p", LOW, BASE + 1799), Suppress);
        assert_eq!(g.peek("p", LOW, BASE + 1800), Run);
    }

    #[test]
    fn window_is_capped() {
        let mut g = ReclaimEffectivenessGate::new(1_000, 10, 5_000, CEILING);
        // 1000 → 5000 (capped, not 10_000). Re-record at the window edge so the
        // "silence >= 2× window" reset never trips.
        let mut now = 0;
        g.record("p", false, now); // 1000
        now += 1_000;
        g.record("p", false, now); // 10_000 → capped 5_000
        now += 5_000;
        g.record("p", false, now); // stays capped at 5_000
        assert_eq!(g.peek("p", LOW, now + 4_999), Suppress);
        assert_eq!(g.peek("p", LOW, now + 5_000), Run);
    }

    #[test]
    fn long_silence_resets_to_base() {
        let mut g = gate();
        g.record("p", false, 0);
        g.record("p", false, BASE); // window → 1800, streak 2
        // Silence >= 2× current window (2*1800=3600) ⇒ reset to base + streak 1.
        g.record("p", false, BASE + 3_600);
        assert_eq!(g.noop_streak("p"), 1);
        assert_eq!(g.peek("p", LOW, BASE + 3_600 + BASE - 1), Suppress);
        assert_eq!(g.peek("p", LOW, BASE + 3_600 + BASE), Run);
    }

    // ── record: effective reset ──────────────────────────────────────────────

    #[test]
    fn effective_run_readmits_next_cycle_immediately() {
        // The invariant: an effective run must NOT arm a cooldown — the very next
        // peek (same instant) re-admits.
        let mut g = gate();
        g.record("p", false, 0); // arm cooldown
        assert_eq!(g.peek("p", LOW, 1), Suppress);
        g.record("p", true, 1); // freed space
        assert_eq!(g.peek("p", LOW, 1), Run, "effective ⇒ immediate re-admit");
        assert_eq!(g.noop_streak("p"), 0);
    }

    #[test]
    fn effective_run_after_streak_clears_streak() {
        let mut g = gate();
        g.record("p", false, 0);
        g.record("p", false, BASE);
        assert_eq!(g.noop_streak("p"), 2);
        g.record("p", true, BASE + 10);
        assert_eq!(g.noop_streak("p"), 0);
    }

    // ── independence & robustness ────────────────────────────────────────────

    #[test]
    fn keys_back_off_independently() {
        let mut g = gate();
        g.record("a", false, 0);
        // "b" is unseen ⇒ runs; "a" is in cooldown ⇒ suppressed.
        assert_eq!(g.peek("a", LOW, 1), Suppress);
        assert_eq!(g.peek("b", LOW, 1), Run);
    }

    #[test]
    fn growth_saturates_without_panic() {
        // Enormous multiplier and window near i64::MAX must not overflow-panic;
        // saturating_mul + cap keep it bounded.
        let mut g = ReclaimEffectivenessGate::new(i64::MAX - 1, i64::MAX, i64::MAX, CEILING);
        g.record("p", false, 0);
        g.record("p", false, 1);
        // No panic; still a valid (suppressing) window.
        assert_eq!(g.peek("p", LOW, 2), Suppress);
    }

    #[test]
    fn new_floors_multiplier_and_windows() {
        // A multiplier < 2 would never grow the window; `new` floors it to 2.
        let mut g = ReclaimEffectivenessGate::new(0, 1, 0, CEILING);
        g.record("p", false, 0);
        // base floored to 1, so it suppresses within [0,1) and grows thereafter.
        assert_eq!(g.peek("p", LOW, 0), Suppress);
        assert_eq!(g.peek("p", LOW, 1), Run);
    }

    // ── suppress-only invariant ──────────────────────────────────────────────

    #[test]
    fn decision_is_binary_run_or_suppress() {
        // The gate can only ever decide Run/Suppress — it has no surface to turn a
        // dry-run into an apply. Exhaustive match documents that closed set.
        for d in [Run, Suppress] {
            match d {
                Run | Suppress => {}
            }
        }
    }

    // ── configuration ────────────────────────────────────────────────────────

    #[test]
    fn gate_enabled_defaults_on() {
        assert!(effectiveness_gate_enabled_from(env(&[])));
        assert!(effectiveness_gate_enabled_from(env(&[(
            EFFECTIVENESS_GATE_ENV,
            "on"
        )])));
        assert!(effectiveness_gate_enabled_from(env(&[(
            EFFECTIVENESS_GATE_ENV,
            "garbage"
        )])));
    }

    #[test]
    fn gate_disabled_only_by_explicit_falsey() {
        for v in ["off", "OFF", "0", "false", "no", " No "] {
            assert!(
                !effectiveness_gate_enabled_from(env(&[(EFFECTIVENESS_GATE_ENV, v)])),
                "value {v:?} should disable the gate"
            );
        }
    }

    #[test]
    fn cooldown_base_defaults_and_validates() {
        assert_eq!(
            cooldown_base_secs_from(env(&[])),
            DEFAULT_COOLDOWN_BASE_SECS
        );
        assert_eq!(
            cooldown_base_secs_from(env(&[(COOLDOWN_BASE_SECS_ENV, "120")])),
            120
        );
        assert_eq!(
            cooldown_base_secs_from(env(&[(COOLDOWN_BASE_SECS_ENV, "0")])),
            DEFAULT_COOLDOWN_BASE_SECS,
            "non-positive falls back to default"
        );
        assert_eq!(
            cooldown_base_secs_from(env(&[(COOLDOWN_BASE_SECS_ENV, "nope")])),
            DEFAULT_COOLDOWN_BASE_SECS
        );
    }

    #[test]
    fn cooldown_multiplier_clamps_to_two() {
        assert_eq!(
            cooldown_multiplier_from(env(&[])),
            DEFAULT_COOLDOWN_MULTIPLIER
        );
        assert_eq!(
            cooldown_multiplier_from(env(&[(COOLDOWN_MULTIPLIER_ENV, "5")])),
            5
        );
        assert_eq!(
            cooldown_multiplier_from(env(&[(COOLDOWN_MULTIPLIER_ENV, "1")])),
            2,
            "sub-2 clamps to 2"
        );
        assert_eq!(
            cooldown_multiplier_from(env(&[(COOLDOWN_MULTIPLIER_ENV, "-9")])),
            2
        );
    }

    #[test]
    fn cooldown_max_defaults_and_validates() {
        assert_eq!(cooldown_max_secs_from(env(&[])), DEFAULT_COOLDOWN_MAX_SECS);
        assert_eq!(
            cooldown_max_secs_from(env(&[(COOLDOWN_MAX_SECS_ENV, "60")])),
            60
        );
        assert_eq!(
            cooldown_max_secs_from(env(&[(COOLDOWN_MAX_SECS_ENV, "0")])),
            DEFAULT_COOLDOWN_MAX_SECS
        );
    }

    #[test]
    fn hard_ceiling_defaults_and_clamps() {
        assert_eq!(hard_ceiling_pct_from(env(&[])), DEFAULT_HARD_CEILING_PCT);
        assert_eq!(
            hard_ceiling_pct_from(env(&[(HARD_CEILING_PCT_ENV, "95")])),
            95
        );
        assert_eq!(
            hard_ceiling_pct_from(env(&[(HARD_CEILING_PCT_ENV, "0")])),
            1,
            "clamps up to 1"
        );
        assert_eq!(
            hard_ceiling_pct_from(env(&[(HARD_CEILING_PCT_ENV, "250")])),
            100,
            "valid u8 above 100 clamps down to 100"
        );
        assert_eq!(
            hard_ceiling_pct_from(env(&[(HARD_CEILING_PCT_ENV, "300")])),
            DEFAULT_HARD_CEILING_PCT,
            "u8 overflow ⇒ parse fails ⇒ default"
        );
    }
}
