//! The amplihack freshness gate.
//!
//! Simard runs `amplihack update` immediately before launching each engineer
//! subprocess (and once at daemon startup) so every engineer runs on the LATEST
//! `amplihack-rs` — its recipes, recipe-runner, and SDK adapters. A stale
//! installed bundle once carried per-step agent timeouts that upstream had
//! already removed, and those stale timeouts killed working agent steps; the
//! gate exists so that can never recur.
//!
//! The gate is:
//! - **serialized + deduped** — a cross-process `flock(2)` lets only one update
//!   run at a time, and a durable TTL skips a redundant rebuild when a
//!   successful update is recent;
//! - **honestly surfaced, never silent** — a failed update logs at warn/error
//!   and records the `amplihack_update_failure` metric, then by default proceeds
//!   on the last-known-good install, or (under `SIMARD_REQUIRE_FRESH_AMPLIHACK=1`)
//!   refuses the spawn with an explicit error;
//! - **liveness-bounded, never wall-clock killed** — the update subprocess is
//!   only aborted when it stops making progress, and any such expiry is surfaced
//!   as a `failed` outcome.
//!
//! See `docs/reference/amplihack-freshness-gate.md` for the authoritative
//! contract and `docs/concepts/amplihack-freshness-gate.md` for the rationale.

mod gate;
mod runner;

#[cfg(test)]
mod tests;

pub use gate::{
    AmplihackUpdater, DEFAULT_TTL_SECS, ENV_ENABLED, ENV_REQUIRE_FRESH, ENV_TTL, FAILURE_METRIC,
    GateClock, GateConfig, GateOutcome, MetricSink, TRACE_TARGET, UPDATE_LOCK_FILENAME,
    UPDATE_STATE_FILENAME, run_freshness_gate,
};
pub use runner::{
    RealUpdater, SelfMetricsSink, SystemClock, ensure_amplihack_fresh, ensure_amplihack_fresh_in,
};
