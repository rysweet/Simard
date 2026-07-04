//! The [`CognitiveThread`] trait and its supporting data contracts.
//!
//! These are the "studs" from Appendix A of the design doc. The types are the
//! stable data surface; only the *behaviour* (in `schedule`, `mind`, and each
//! `threads::*` module) is stubbed during TDD.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::cognitive_memory::CognitiveMemoryOps;

/// A single scheduled mental process owned by the [`super::Mind`].
///
/// Object-safe (`Mind` stores `Box<dyn CognitiveThread>`) and `Send` (threads
/// are moved into the `Mind` at registration; the scheduler ticks them
/// sequentially on one thread, so `Sync` is not required).
pub trait CognitiveThread: Send {
    /// Stable, unique, `snake_case` id used in telemetry metric/span names.
    fn id(&self) -> &str;

    /// Human-facing name for logs/dashboard. Defaults to [`Self::id`].
    fn name(&self) -> &str {
        self.id()
    }

    /// Coarse class of process.
    fn kind(&self) -> ThreadKind;

    /// When this thread wants to run.
    fn policy(&self) -> SchedulePolicy;

    /// Priority / resource class. OODA is always [`Priority::Critical`].
    fn priority(&self) -> Priority {
        Priority::Normal
    }

    /// Runtime enable/disable (e.g. env-gated). Disabled threads never tick.
    fn enabled(&self) -> bool {
        true
    }

    /// Execute exactly one step. MUST be best-effort and self-contained: return
    /// [`ThreadOutcome::failed`] rather than panic where possible; the `Mind`
    /// also catches panics as a backstop. May `block_on` `ctx.runtime`.
    fn tick(&mut self, ctx: &mut ThreadContext<'_>) -> ThreadOutcome;

    /// Current health/heartbeat snapshot (last-run, next-run, last outcome).
    fn health(&self) -> ThreadHealth;
}

/// Coarse class of a cognitive process.
///
/// The first three variants are implemented in this PR; the rest are reserved
/// for the "mind of many processes" vision — the same [`super::Mind`] can host
/// them later without a trait change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum ThreadKind {
    /// The primary active OODA loop (implemented).
    Ooda,
    /// Housekeeping / cleanup (implemented, exemplar 1).
    Maintenance,
    /// Engineer-log improvement finder (implemented, exemplar 2).
    EngineerLogAnalysis,
    /// Reserved: idle associative background thought.
    BackgroundThought,
    /// Reserved: sleep/dream memory consolidation.
    MemoryConsolidation,
    /// Reserved: sensory pre-processing.
    SensoryProcessing,
    /// Reserved: long-horizon planning.
    LongTermPlanning,
}

/// How a thread decides it is due to run.
#[derive(Clone, Debug, PartialEq)]
pub enum SchedulePolicy {
    /// Fixed cadence: `next_run = last_run + interval`.
    Interval(Duration),
    /// Only when explicitly requested (operator/event). Never auto-due.
    OnDemand,
    /// Due when an external predicate fires (a flag/predicate on the context).
    EventDriven,
    /// Cadence adapts to load/outcome within `min..=max`. Reserved; for now it
    /// behaves as `Interval(current)` so it is representable but conservative.
    Adaptive {
        /// Lower cadence bound.
        min: Duration,
        /// Upper cadence bound.
        max: Duration,
        /// Current effective cadence.
        current: Duration,
    },
}

/// Priority / resource class. Ordered so ascending sort places OODA first:
/// `Critical < High < Normal < Low`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum Priority {
    /// OODA only — never budgeted, never starved, never backed off.
    Critical,
    /// High-priority background work.
    High,
    /// Default class.
    Normal,
    /// Slow, best-effort housekeeping (maintenance, analysis).
    Low,
}

/// Typed result of a single [`CognitiveThread::tick`].
#[derive(Clone, Debug, serde::Serialize)]
pub struct ThreadOutcome {
    /// `false` => the thread was not due / skipped this tick.
    pub ran: bool,
    /// Whether the run succeeded.
    pub success: bool,
    /// Structured, human-readable summary.
    pub summary: String,
    /// Wall-clock duration of the run.
    pub duration: Duration,
    /// Thread-specific structured fields (never a snapshot doc).
    pub detail: serde_json::Value,
}

impl ThreadOutcome {
    /// The thread was not due / did no work this tick.
    pub fn skipped() -> Self {
        Self {
            ran: false,
            success: true,
            summary: String::new(),
            duration: Duration::ZERO,
            detail: serde_json::Value::Null,
        }
    }

    /// A successful run.
    pub fn ok(summary: impl Into<String>, duration: Duration) -> Self {
        Self {
            ran: true,
            success: true,
            summary: summary.into(),
            duration,
            detail: serde_json::Value::Null,
        }
    }

    /// A failed run.
    pub fn failed(summary: impl Into<String>, duration: Duration) -> Self {
        Self {
            ran: true,
            success: false,
            summary: summary.into(),
            duration,
            detail: serde_json::Value::Null,
        }
    }

    /// Attach structured detail (builder style).
    #[must_use]
    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = detail;
        self
    }
}

/// Health/heartbeat snapshot for the dashboard and diagnostics.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ThreadHealth {
    /// Stable thread id.
    pub id: String,
    /// Whether the thread is currently enabled.
    pub enabled: bool,
    /// Unix epoch (seconds) of the last completed run, if any.
    pub last_run_epoch: Option<u64>,
    /// Unix epoch (seconds) of the next scheduled run, if computable.
    pub next_run_epoch: Option<u64>,
    /// Success flag of the most recent run, if any.
    pub last_success: Option<bool>,
    /// Consecutive error count (drives backoff).
    pub consecutive_errors: u32,
    /// If backed off, the epoch until which the thread is suppressed.
    pub backoff_until_epoch: Option<u64>,
}

/// Borrowed daemon resources handed to each tick so threads do not reach into
/// globals. `now_epoch` is an *injected* clock so due-computation and backoff
/// are unit-testable with no sleeps.
pub struct ThreadContext<'a> {
    /// `~/.simard` state root.
    pub state_root: &'a Path,
    /// Repository root.
    pub repo_root: &'a Path,
    /// Live cognitive-store handle (`: Send + Sync`).
    pub memory: &'a dyn CognitiveMemoryOps,
    /// Runtime handle for `block_on`-ing async work inside a tick.
    pub runtime: tokio::runtime::Handle,
    /// Cooperative cancellation flag checked by the scheduler and long ticks.
    pub shutdown: &'a AtomicBool,
    /// Injected unix-epoch-seconds clock.
    pub now_epoch: u64,
    /// Global safety switch (dry-run everything destructive).
    pub dry_run: bool,
}
