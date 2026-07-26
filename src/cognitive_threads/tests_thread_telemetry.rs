//! TDD contract (Step 7) for cognitive-thread observability instrumentation
//! (issue #4786). These tests are authored **before** the instrumentation
//! exists, so the module is RED until the builder ships:
//!
//!   * `telemetry::record_run` gains a `run_epoch` arg and **dual-writes**
//!     through the shared OTel facade the per-thread `runs` / `successes` /
//!     `failures` counters, the `duration_seconds` histogram, and the
//!     `last_run_epoch` gauge (design C1 / requirement R1).
//!   * `telemetry::record_next_run` dual-writes the `next_run_epoch` gauge.
//!   * `Mind::execute`, on a non-success tick, bumps the `failures` counter
//!     **and** records a durable `FailureDiagnosis { cause:
//!     FailureCause::CognitiveThread, .. }` via the Overseer `failure_sink`
//!     (design C6 / requirement R6).
//!   * `signal::signals_from` lifts a drained thread `FailureDiagnosis` into a
//!     `Signal::StepFailureDiagnosed` (requirements R6/R7).
//!
//! The metrics registry and the failure sink are process-global, so the tests
//! that touch them are `#[serial]` and reset/drain their global state up-front.
//! Metric identity is embedded in the **name** (`simard.thread.<id>.<suffix>`,
//! design A3) so every series is asserted with an EMPTY attribute set.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serial_test::serial;

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::overseer::capabilities::ObservedState;
use crate::overseer::diagnosis::{FailureCause, FailureDiagnosis};
use crate::overseer::failure_sink;
use crate::overseer::signal::{self, Signal};
use crate::telemetry::registry;

use super::mind::Mind;
use super::telemetry as ct_telemetry;
use super::thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};

// ---------------------------------------------------------------------------
// C10 / R1 — the telemetry seam dual-writes the per-thread series.
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn record_run_success_dual_writes_thread_series() {
    registry::reset();
    let run_epoch = 1_700_000_000_u64;
    let outcome = ThreadOutcome::ok("distilled 3 facts", Duration::from_millis(250));

    ct_telemetry::record_run("distill", &outcome, run_epoch);
    let snap = registry::capture();

    assert_eq!(
        snap.counter(&ct_telemetry::metric_name("distill", "runs"), &[]),
        Some(1),
        "every attempt bumps the runs counter"
    );
    assert_eq!(
        snap.counter(&ct_telemetry::metric_name("distill", "successes"), &[]),
        Some(1),
        "a successful tick bumps the successes counter"
    );
    assert_eq!(
        snap.counter(&ct_telemetry::metric_name("distill", "failures"), &[])
            .unwrap_or(0),
        0,
        "a successful tick must NOT bump the failures counter"
    );

    let hist = snap
        .histogram(
            &ct_telemetry::metric_name("distill", "duration_seconds"),
            &[],
        )
        .expect("duration_seconds histogram series present after a run");
    assert_eq!(hist.count, 1, "exactly one duration observation recorded");
    assert!(
        (hist.sum - 0.25).abs() < 1e-6,
        "histogram sum reflects the real 250ms run, got {}",
        hist.sum
    );

    assert_eq!(
        snap.gauge(&ct_telemetry::metric_name("distill", "last_run_epoch"), &[]),
        Some(run_epoch as i64),
        "last_run_epoch gauge reflects the real run time (no hardcoding)"
    );
}

#[test]
#[serial]
fn record_run_failure_increments_failures_only() {
    registry::reset();
    let outcome = ThreadOutcome::failed("boom", Duration::from_millis(5));

    ct_telemetry::record_run("reflection", &outcome, 1_700_000_100);
    let snap = registry::capture();

    assert_eq!(
        snap.counter(&ct_telemetry::metric_name("reflection", "runs"), &[]),
        Some(1),
        "a failed tick still counts as an attempt"
    );
    assert_eq!(
        snap.counter(&ct_telemetry::metric_name("reflection", "failures"), &[]),
        Some(1),
        "a failed tick bumps the failures counter"
    );
    assert_eq!(
        snap.counter(&ct_telemetry::metric_name("reflection", "successes"), &[])
            .unwrap_or(0),
        0,
        "a failed tick must NOT bump the successes counter"
    );
}

#[test]
#[serial]
fn record_next_run_sets_next_run_epoch_gauge() {
    registry::reset();

    ct_telemetry::record_next_run("planning", Some(1_700_000_600));
    let snap = registry::capture();

    assert_eq!(
        snap.gauge(
            &ct_telemetry::metric_name("planning", "next_run_epoch"),
            &[]
        ),
        Some(1_700_000_600),
        "next_run_epoch gauge is the liveness/staleness seam the Overseer reads"
    );
}

// ---------------------------------------------------------------------------
// C6 / R6 — a failing tick is observable on BOTH channels (counter + durable
// diagnosis) end-to-end through the real scheduler.
// ---------------------------------------------------------------------------

/// A `CognitiveThread` whose tick always fails — the minimal driver for the
/// error-propagation contract.
struct FailingThread {
    id: String,
}

impl CognitiveThread for FailingThread {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> ThreadKind {
        ThreadKind::Maintenance
    }
    fn policy(&self) -> SchedulePolicy {
        SchedulePolicy::Interval(Duration::ZERO)
    }
    fn priority(&self) -> Priority {
        Priority::Low
    }
    fn tick(&mut self, _ctx: &mut ThreadContext<'_>) -> ThreadOutcome {
        ThreadOutcome::failed("boom: simulated thread error", Duration::from_millis(2))
    }
    fn health(&self) -> ThreadHealth {
        ThreadHealth {
            id: self.id.clone(),
            enabled: true,
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
            consecutive_errors: 0,
            backoff_until_epoch: None,
            // Registry single-source-of-truth fields (design C3).
            purpose: "simulate a failing cognitive thread".to_string(),
            cadence_secs: Some(0),
        }
    }
}

#[test]
#[serial]
fn failing_thread_bumps_failures_counter_and_records_durable_diagnosis() {
    registry::reset();
    // Clear any residue a prior test left in the process-global sink.
    let _ = failure_sink::drain_recent();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime");
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory cognitive store");
    let tmp = tempfile::tempdir().expect("tempdir");
    let shutdown = AtomicBool::new(false);

    let mut mind = Mind::with_budget(4);
    mind.register(Box::new(FailingThread {
        id: "flaky".to_string(),
    }));

    let mut ctx = ThreadContext {
        state_root: tmp.path() as &Path,
        repo_root: tmp.path(),
        memory: &mem as &dyn CognitiveMemoryOps,
        runtime: rt.handle().clone(),
        shutdown: &shutdown,
        now_epoch: 1_700_000_000,
        dry_run: true,
    };

    let outcomes = mind.run_due(&mut ctx);
    assert_eq!(outcomes.len(), 1, "the one due thread ran");
    assert!(!outcomes[0].success, "the thread reported failure");

    // Channel 1 — the per-thread OTel failures counter.
    let snap = registry::capture();
    assert_eq!(
        snap.counter(&ct_telemetry::metric_name("flaky", "failures"), &[]),
        Some(1),
        "a failed tick increments the per-thread failures counter"
    );

    // Channel 2 — the durable, Overseer-drained failure diagnosis.
    let drained = failure_sink::drain_recent();
    let diag = drained
        .iter()
        .find(|d| d.cause == FailureCause::CognitiveThread)
        .expect("a failed thread records a durable CognitiveThread FailureDiagnosis");
    assert!(
        diag.evidence.contains("boom") || diag.evidence.contains("flaky"),
        "diagnosis evidence carries the real failure summary / thread id, got: {}",
        diag.evidence
    );
}

// ---------------------------------------------------------------------------
// R6 / R7 — a drained thread failure lifts into a corrective Overseer signal.
// ---------------------------------------------------------------------------

#[test]
fn drained_thread_failure_becomes_step_failure_signal() {
    let diag = FailureDiagnosis {
        cause: FailureCause::CognitiveThread,
        exit_code: None,
        evidence: "thread flaky: boom".to_string(),
    };
    let state = ObservedState {
        recent_step_failures: vec![diag],
        ..ObservedState::default()
    };

    let signals = signal::signals_from(&state);
    let lifted = signals.iter().any(|s| {
        matches!(
            s,
            Signal::StepFailureDiagnosed {
                cause: FailureCause::CognitiveThread,
                ..
            }
        )
    });
    assert!(
        lifted,
        "a drained CognitiveThread failure must surface as Signal::StepFailureDiagnosed"
    );
}
