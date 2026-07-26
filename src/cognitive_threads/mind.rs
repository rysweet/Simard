//! The [`Mind`] — the cognitive-thread scheduler (Appendix A.5).
//!
//! Owns a registry of threads, computes which are due, and runs them under a
//! priority budget that never starves OODA. Failure isolation, backoff, and
//! graceful shutdown live in [`Mind::run_due`]; the registry/bookkeeping
//! surface is the stable "stud" the tests in `super::tests` build against.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::schedule;
use super::telemetry;
use super::thread::{CognitiveThread, Priority, ThreadContext, ThreadHealth, ThreadOutcome};

/// Env var controlling the per-tick non-critical fan-out budget.
const BUDGET_ENV: &str = "SIMARD_MIND_MAX_NONCRITICAL_PER_TICK";
/// Default non-critical fan-out per tick when the env var is unset/invalid.
const DEFAULT_BUDGET: usize = 2;

/// Base delay of the per-thread capped-exponential backoff.
const BACKOFF_BASE: Duration = Duration::from_secs(30);
/// Ceiling of the per-thread backoff — a wedged thread retries at most this
/// slowly, never permanently silenced.
const BACKOFF_CAP: Duration = Duration::from_secs(30 * 60);

/// Per-thread runtime bookkeeping held alongside the boxed thread. This is the
/// scheduler's authoritative view (a thread's own `health()` is advisory); the
/// dashboard heartbeat is built from these fields.
struct ThreadEntry {
    thread: Box<dyn CognitiveThread>,
    last_run: Option<u64>,
    next_run: Option<u64>,
    last_success: Option<bool>,
    consecutive_errors: u32,
    backoff_until: Option<u64>,
}

impl ThreadEntry {
    fn new(thread: Box<dyn CognitiveThread>) -> Self {
        Self {
            thread,
            last_run: None,
            next_run: None,
            last_success: None,
            consecutive_errors: 0,
            backoff_until: None,
        }
    }

    /// Whether this thread is currently suppressed by backoff at `now`.
    fn is_backed_off(&self, now: u64) -> bool {
        self.backoff_until.is_some_and(|until| now < until)
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
    /// threads at `now_epoch`. `Critical` threads are cadence-driven like any
    /// other here; the budget/never-backed-off exemptions live in
    /// [`Mind::run_due`].
    pub fn due_threads(&self, now_epoch: u64) -> Vec<usize> {
        (0..self.threads.len())
            .filter(|&i| {
                let e = &self.threads[i];
                e.thread.enabled()
                    && !e.is_backed_off(now_epoch)
                    && schedule::is_due(&e.thread.policy(), e.last_run, now_epoch)
            })
            .collect()
    }

    /// Run OODA ([`Priority::Critical`]) first and unconditionally (budget-
    /// exempt, never backed off), then non-critical due threads in priority
    /// order up to the per-tick budget. Each tick runs inside `catch_unwind`;
    /// a panic/`Err` bumps `consecutive_errors`, sets backoff, emits an error
    /// metric, and never propagates. Once shutdown is requested no new ticks
    /// start (the in-flight inline OODA cycle drains via the daemon path).
    pub fn run_due(&mut self, ctx: &mut ThreadContext<'_>) -> Vec<ThreadOutcome> {
        let now = ctx.now_epoch;
        let mut outcomes = Vec::new();

        // Graceful shutdown: start nothing new.
        if ctx.shutdown.load(Ordering::SeqCst) {
            return outcomes;
        }

        // Phase 1 — Critical (OODA) first. Cadence-respecting, but budget-exempt
        // and never backed off so a flood of due Low threads can never starve
        // it and a reported failure never suppresses it.
        for i in 0..self.threads.len() {
            let e = &self.threads[i];
            if e.thread.priority() == Priority::Critical
                && e.thread.enabled()
                && schedule::is_due(&e.thread.policy(), e.last_run, now)
            {
                let outcome = Self::execute(&mut self.threads[i], ctx, now, true);
                outcomes.push(outcome);
            }
        }

        // Re-check shutdown between phases (a signal may have arrived during a
        // long OODA tick); drain rather than start background work.
        if ctx.shutdown.load(Ordering::SeqCst) {
            return outcomes;
        }

        // Phase 2 — non-critical due threads, priority-ordered, bounded.
        let mut due: Vec<usize> = (0..self.threads.len())
            .filter(|&i| {
                let e = &self.threads[i];
                e.thread.priority() != Priority::Critical
                    && e.thread.enabled()
                    && !e.is_backed_off(now)
                    && schedule::is_due(&e.thread.policy(), e.last_run, now)
            })
            .collect();
        // Stable sort by ascending priority (Critical < High < Normal < Low)
        // keeps registration order among equals — no starvation reordering.
        due.sort_by_key(|&i| self.threads[i].thread.priority());

        // Cap non-critical fan-out at the per-tick budget; `take` keeps the
        // limit without a manual counter (clippy::explicit_counter_loop).
        for i in due.into_iter().take(self.budget.max_noncritical_per_tick) {
            if ctx.shutdown.load(Ordering::SeqCst) {
                break;
            }
            let outcome = Self::execute(&mut self.threads[i], ctx, now, false);
            outcomes.push(outcome);
        }

        outcomes
    }

    /// Execute one thread with panic isolation and update its bookkeeping.
    ///
    /// `critical` threads never accrue backoff/error counts (OODA must keep its
    /// cadence even if a cycle reports failure).
    fn execute(
        entry: &mut ThreadEntry,
        ctx: &mut ThreadContext<'_>,
        now: u64,
        critical: bool,
    ) -> ThreadOutcome {
        let id = entry.thread.id().to_string();
        let _active = telemetry::enter_active(&id);

        // AssertUnwindSafe: a caught panic leaves the thread's private state
        // possibly inconsistent, but we immediately back it off and never read
        // that state until it succeeds again — the isolation is the contract.
        let result = catch_unwind(AssertUnwindSafe(|| entry.thread.tick(ctx)));
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(_) => ThreadOutcome::failed("thread panicked", Duration::ZERO),
        };

        entry.last_run = Some(now);
        entry.last_success = Some(outcome.success);
        entry.next_run = schedule::next_run_epoch(&entry.thread.policy(), entry.last_run, now);

        telemetry::record_run(&id, &outcome, now);
        telemetry::record_next_run(&id, entry.next_run);

        if outcome.success {
            entry.consecutive_errors = 0;
            entry.backoff_until = None;
        } else {
            // Errors flow to the Overseer on BOTH channels (issue #4786): the
            // per-thread `failures` counter (bumped inside `record_run`) AND a
            // durable, Overseer-drained `FailureDiagnosis`, so a caught thread
            // failure drives a corrective `Signal::StepFailureDiagnosed` instead
            // of being swallowed inside the thread.
            telemetry::record_error(&id, &outcome.summary);
            record_thread_failure(&id, &outcome.summary);
            if !critical {
                entry.consecutive_errors = entry.consecutive_errors.saturating_add(1);
                entry.backoff_until = Some(schedule::backoff_until_epoch(
                    now,
                    entry.consecutive_errors,
                    BACKOFF_BASE,
                    BACKOFF_CAP,
                ));
            }
        }

        outcome
    }

    /// Health snapshot of every registered thread (dashboard heartbeat feed).
    /// Built from the scheduler's authoritative bookkeeping, not the thread's
    /// own advisory `health()`.
    pub fn health(&self) -> Vec<ThreadHealth> {
        self.threads
            .iter()
            .map(|e| ThreadHealth {
                id: e.thread.id().to_string(),
                enabled: e.thread.enabled(),
                last_run_epoch: e.last_run,
                next_run_epoch: e.next_run,
                last_success: e.last_success,
                consecutive_errors: e.consecutive_errors,
                backoff_until_epoch: e.backoff_until,
                // Single source of truth for the Overseer's thread registry
                // (#4786): purpose from the thread itself, cadence derived from
                // its schedule policy — no hand-maintained duplicate list.
                purpose: e.thread.purpose().to_string(),
                cadence_secs: e.thread.policy().cadence_secs(),
            })
            .collect()
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

/// Record a durable [`FailureDiagnosis`] for a failed cognitive-thread tick into
/// the process-global Overseer [`failure_sink`], so the failure surfaces as a
/// corrective `Signal::StepFailureDiagnosed` on the next Observe pass (issue
/// #4786). Evidence carries the thread id and its summary, bounded to
/// [`FailureDiagnosis`]'s evidence cap so a pathological summary can never
/// inflate the sink or a downstream notification.
fn record_thread_failure(id: &str, summary: &str) {
    use crate::overseer::diagnosis::{FailureCause, FailureDiagnosis, MAX_EVIDENCE_LEN};
    use crate::overseer::failure_sink;

    let raw = format!("thread {id}: {summary}");
    let evidence: String = if raw.chars().count() <= MAX_EVIDENCE_LEN {
        raw
    } else {
        raw.chars().take(MAX_EVIDENCE_LEN).collect()
    };
    failure_sink::record_step_failure(FailureDiagnosis {
        cause: FailureCause::CognitiveThread,
        exit_code: None,
        evidence,
    });
}
