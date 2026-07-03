//! The [`Mind`] — the cognitive-thread scheduler (Appendix A.5).
//!
//! Owns a registry of threads, computes which are due, and runs them under a
//! priority budget that never starves OODA. Failure isolation, backoff, and
//! graceful shutdown live in [`Mind::run_due`], whose body is a `todo!()`
//! stub during TDD; the registry/bookkeeping surface is real so the tests in
//! `super::tests` can build a `Mind` and register fakes.
#![allow(dead_code, unused_variables)]

use super::thread::{CognitiveThread, ThreadContext, ThreadHealth, ThreadOutcome};

/// Env var controlling the per-tick non-critical fan-out budget.
const BUDGET_ENV: &str = "SIMARD_MIND_MAX_NONCRITICAL_PER_TICK";
/// Default non-critical fan-out per tick when the env var is unset/invalid.
const DEFAULT_BUDGET: usize = 2;

/// Per-thread runtime bookkeeping held alongside the boxed thread.
struct ThreadEntry {
    thread: Box<dyn CognitiveThread>,
    last_run: Option<u64>,
    next_run: Option<u64>,
    consecutive_errors: u32,
    backoff_until: Option<u64>,
}

impl ThreadEntry {
    fn new(thread: Box<dyn CognitiveThread>) -> Self {
        Self {
            thread,
            last_run: None,
            next_run: None,
            consecutive_errors: 0,
            backoff_until: None,
        }
    }
}

/// Per-tick run budget for non-critical threads (OODA is exempt).
struct RunBudget {
    max_noncritical_per_tick: usize,
}

/// The cognitive-thread scheduler.
pub struct Mind {
    threads: Vec<ThreadEntry>,
    budget: RunBudget,
}

impl Mind {
    /// Build a `Mind` with the budget read from [`BUDGET_ENV`] (default
    /// [`DEFAULT_BUDGET`]).
    pub fn new() -> Self {
        let max = std::env::var(BUDGET_ENV)
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_BUDGET);
        Self::with_budget(max)
    }

    /// Build a `Mind` with an explicit non-critical per-tick budget (test seam
    /// — avoids env mutation).
    pub fn with_budget(max_noncritical_per_tick: usize) -> Self {
        Self {
            threads: Vec::new(),
            budget: RunBudget {
                max_noncritical_per_tick,
            },
        }
    }

    /// Register a thread (chainable).
    pub fn register(&mut self, thread: Box<dyn CognitiveThread>) -> &mut Self {
        self.threads.push(ThreadEntry::new(thread));
        self
    }

    /// Pure: registration-order indices of enabled, non-backed-off, due
    /// threads at `now_epoch`.
    pub fn due_threads(&self, now_epoch: u64) -> Vec<usize> {
        todo!("Step 7 TDD: implemented by the scheduler implementation step")
    }

    /// Run OODA ([`super::Priority::Critical`]) first and unconditionally
    /// (budget-exempt, never backed off), then non-critical due threads in
    /// priority order up to the per-tick budget. Each tick runs inside
    /// `catch_unwind`; a panic/`Err` bumps `consecutive_errors`, sets backoff,
    /// emits an error metric, and never propagates. Checks `ctx.shutdown`
    /// between threads and returns early.
    pub fn run_due(&mut self, ctx: &mut ThreadContext<'_>) -> Vec<ThreadOutcome> {
        todo!("Step 7 TDD: implemented by the scheduler implementation step")
    }

    /// Health snapshot of every registered thread (dashboard heartbeat feed).
    pub fn health(&self) -> Vec<ThreadHealth> {
        todo!("Step 7 TDD: implemented by the scheduler implementation step")
    }

    /// Number of registered threads.
    pub fn len(&self) -> usize {
        self.threads.len()
    }

    /// Whether no threads are registered.
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }
}

impl Default for Mind {
    fn default() -> Self {
        Self::new()
    }
}
