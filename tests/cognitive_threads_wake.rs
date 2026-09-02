//! Integration TDD contract (Step 7) for cognitive-thread FULL ACTIVATION
//! (issue #4845). Builds on the merged #4786 telemetry facade + Overseer
//! thread-oversight + failure sink. Authored BEFORE the activation change, so
//! the default-ON / budget / roster assertions are RED until Phases 1–3 ship;
//! they reference only EXISTING public APIs so the crate still compiles (true
//! RED: it builds and fails on the new behaviour), not a compile break.
//!
//! Coverage (success criteria from the brief):
//!   1. Registration is DEFAULT-ON: the whole reflective roster is ENABLED with
//!      no env set; per-thread `SIMARD_THREAD_<X>_ENABLED=0` opts out exactly
//!      that thread; the master `SIMARD_COGNITIVE_THREADS_ENABLED=0` opts out
//!      the roster. (design C1/C2)
//!   2. The scheduler runs N background threads (budget default covers the full
//!      scheduled non-critical roster, not 2). (design C3)
//!   3. `simard.thread.*` telemetry is present for EVERY registered thread after
//!      a tick. (design C5 / requirement 2)
//!   4. An injected thread failure is DUAL-ROUTED (failures counter + durable
//!      diagnosis), DETECTED off real telemetry, and folds into EXACTLY ONE
//!      deduplicated remediation dispatch carrying a notifiable summary.
//!      (design C6 / requirements 3–4)
//!   5. Security regressions T-S2..T-S5 (exec-boundary sanitisation + fencing,
//!      path-traversal rejection, remediation-storm cap, no swallowed failures).
//!
//! Env-mutating tests are `#[serial]` and use a scoped guard that restores the
//! managed keys on drop (edition-2024 requires `unsafe` for env mutation).

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serial_test::serial;

use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use simard::cognitive_threads::recipe_rail::{
    UNTRUSTED_CLOSE, UNTRUSTED_OPEN, fence_untrusted, is_fenced_payload, sanitize_value,
    validate_concept_key,
};
use simard::cognitive_threads::threads::register_reflective_threads;
use simard::cognitive_threads::{
    CognitiveThread, EngineerLogAnalysisThread, MaintenanceThread, Mind, Priority, SchedulePolicy,
    ThreadContext, ThreadHealth, ThreadKind, ThreadOutcome,
};
use simard::overseer::capabilities::ObservedState;
use simard::overseer::diagnosis::FailureCause;
use simard::overseer::failure_sink;
use simard::overseer::orient;
use simard::overseer::signal::{self, ProblemKind, Signal};
use simard::overseer::thread_oversight::{MAX_THREAD_ANOMALIES_PER_CYCLE, detect_thread_anomalies};
use simard::telemetry::names;
use simard::telemetry::registry;
use simard::telemetry::snapshot::{CounterSeries, GaugeSeries, MetricsSnapshot};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const MASTER_GATE: &str = "SIMARD_COGNITIVE_THREADS_ENABLED";
const BUDGET_ENV: &str = "SIMARD_MIND_MAX_NONCRITICAL_PER_TICK";
const INTERVAL_SCALE_ENV: &str = "SIMARD_THREAD_INTERVAL_SCALE";
const CREATIVE_GATE: &str = "SIMARD_CREATIVE_IDEAS_ENABLED";

/// The ten reflective threads (issue #5) as (stable id, per-thread gate env),
/// in `register_reflective_threads` order. These are the threads whose
/// `enabled()` reads the DEFAULT-ON opt-out gate the flip changes.
const REFLECTIVE: &[(&str, &str)] = &[
    ("metacognition", "SIMARD_THREAD_METACOGNITION_ENABLED"),
    ("consolidation", "SIMARD_THREAD_CONSOLIDATION_ENABLED"),
    ("reflection", "SIMARD_THREAD_REFLECTION_ENABLED"),
    ("prospection", "SIMARD_THREAD_PROSPECTION_ENABLED"),
    ("salience", "SIMARD_THREAD_SALIENCE_ENABLED"),
    ("operator_model", "SIMARD_THREAD_OPERATOR_MODEL_ENABLED"),
    ("analogy", "SIMARD_THREAD_ANALOGY_ENABLED"),
    ("values_deliberation", "SIMARD_THREAD_VALUES_ENABLED"),
    ("narrative", "SIMARD_THREAD_NARRATIVE_ENABLED"),
    ("interoception", "SIMARD_THREAD_INTEROCEPTION_ENABLED"),
];

/// Every env key these tests touch — snapshotted and restored by [`EnvGuard`] so
/// a test can drive the gates to a known state without leaking into siblings.
fn managed_keys() -> Vec<String> {
    let mut keys = vec![
        MASTER_GATE.to_string(),
        BUDGET_ENV.to_string(),
        INTERVAL_SCALE_ENV.to_string(),
        CREATIVE_GATE.to_string(),
    ];
    keys.extend(REFLECTIVE.iter().map(|(_, gate)| gate.to_string()));
    keys
}

/// Scoped env manager: snapshots the managed keys on construction, lets a test
/// set/remove them, and restores every original on drop (incl. on panic).
struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    /// Snapshot all managed keys and start from a fully-UNSET baseline (the
    /// stock-deployment default the activation change must honour).
    fn clean() -> Self {
        let saved: Vec<(String, Option<String>)> = managed_keys()
            .into_iter()
            .map(|k| {
                let prev = std::env::var(&k).ok();
                // SAFETY: env mutation is serialised by `#[serial]`; every key is
                // restored in `Drop`.
                unsafe { std::env::remove_var(&k) };
                (k, prev)
            })
            .collect();
        Self { saved }
    }

    fn set(&self, key: &str, value: &str) {
        // SAFETY: serialised by `#[serial]`; restored in `Drop`.
        unsafe { std::env::set_var(key, value) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in self.saved.drain(..) {
            // SAFETY: serialised by `#[serial]`; runs on return and on unwind.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var(&k, v),
                    None => std::env::remove_var(&k),
                }
            }
        }
    }
}

/// Owns borrowed resources a [`ThreadContext`] needs.
struct RunEnv {
    rt: tokio::runtime::Runtime,
    mem: LibraryCognitiveMemory,
    shutdown: AtomicBool,
    tmp: tempfile::TempDir,
}

impl RunEnv {
    fn new() -> Self {
        Self {
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime"),
            mem: LibraryCognitiveMemory::in_memory().expect("in-memory store"),
            shutdown: AtomicBool::new(false),
            tmp: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn ctx(&self, now_epoch: u64) -> ThreadContext<'_> {
        ThreadContext {
            state_root: self.tmp.path(),
            repo_root: self.tmp.path(),
            memory: &self.mem as &dyn CognitiveMemoryOps,
            runtime: self.rt.handle().clone(),
            shutdown: &self.shutdown,
            now_epoch,
            // dry_run so every real reflective/maintenance tick short-circuits
            // (no recipe subprocess, no destructive housekeeping).
            dry_run: true,
        }
    }
}

/// Build the master-gated roster the daemon registers: maintenance + engineer-log
/// plus the ten reflective threads (12). Creative-ideas (+1 ⇒ 13 total) rides its
/// own independent gate and is exercised by its own module tests.
fn register_master_gated_roster(mind: &mut Mind, root: &Path) {
    mind.register(Box::new(MaintenanceThread::from_env()));
    mind.register(Box::new(EngineerLogAnalysisThread::from_env()));
    register_reflective_threads(mind, root, root);
}

/// A cognitive thread whose tick always fails — the injected fault.
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
        ThreadOutcome::failed(
            "base type 'ooda-brain' failed during invocation: run_turn failed",
            Duration::from_millis(2),
        )
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
            purpose: "injected failing cognitive thread".to_string(),
            cadence_secs: Some(300),
        }
    }
}

/// An always-due, always-succeeding non-critical thread — drives the budget test.
struct BusyThread {
    id: String,
}

impl CognitiveThread for BusyThread {
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
        ThreadOutcome::ok("ok", Duration::from_millis(1))
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
            purpose: "busy".to_string(),
            cadence_secs: Some(0),
        }
    }
}

// ===========================================================================
// 1. Registration is DEFAULT-ON (opt-out) — success criterion #1.
// ===========================================================================

#[test]
#[serial]
fn roster_is_enabled_by_default_with_no_env_set() {
    let _env = EnvGuard::clean(); // nothing set ⇒ stock deployment
    let tmp = tempfile::tempdir().expect("tempdir");

    let mut mind = Mind::new();
    register_master_gated_roster(&mut mind, tmp.path());

    let health = mind.health();
    assert_eq!(
        health.len(),
        2 + REFLECTIVE.len(),
        "the daemon registers maintenance + engineer-log + all ten reflective threads (12)"
    );
    for h in &health {
        assert!(
            h.enabled,
            "thread '{}' must be ENABLED by default (opt-out) — RED under the old \
             default-OFF double-AND gate where every reflective thread is disabled",
            h.id
        );
    }
    // Every reflective thread is present and on (the ones the flip governs).
    for (id, _) in REFLECTIVE {
        let h = health
            .iter()
            .find(|h| h.id == *id)
            .unwrap_or_else(|| panic!("reflective thread '{id}' registered"));
        assert!(h.enabled, "reflective thread '{id}' enabled by default");
    }
}

#[test]
#[serial]
fn per_thread_env_opts_out_exactly_that_thread() {
    let env = EnvGuard::clean();
    // Opt OUT just metacognition; leave the master + everything else default-ON.
    env.set("SIMARD_THREAD_METACOGNITION_ENABLED", "0");
    let tmp = tempfile::tempdir().expect("tempdir");

    let mut mind = Mind::new();
    register_reflective_threads(&mut mind, tmp.path(), tmp.path());
    let health = mind.health();

    for h in &health {
        if h.id == "metacognition" {
            assert!(
                !h.enabled,
                "SIMARD_THREAD_METACOGNITION_ENABLED=0 must opt OUT metacognition"
            );
        } else {
            assert!(
                h.enabled,
                "opting out one thread must NOT disable '{}' — RED unless the gate is \
                 default-ON opt-out",
                h.id
            );
        }
    }
}

#[test]
#[serial]
fn master_env_opts_out_the_whole_roster() {
    let env = EnvGuard::clean();
    env.set(MASTER_GATE, "0"); // explicit master opt-out
    let tmp = tempfile::tempdir().expect("tempdir");

    let mut mind = Mind::new();
    register_reflective_threads(&mut mind, tmp.path(), tmp.path());

    for h in mind.health() {
        assert!(
            !h.enabled,
            "SIMARD_COGNITIVE_THREADS_ENABLED=0 must disable the whole reflective roster \
             (fail-closed master opt-out); '{}' was still enabled",
            h.id
        );
    }
}

// ===========================================================================
// 2. Scheduler runs N background threads — the budget default covers the roster.
// ===========================================================================

#[test]
#[serial]
fn default_budget_runs_the_whole_scheduled_roster_in_one_tick() {
    let _env = EnvGuard::clean(); // BUDGET_ENV unset ⇒ Mind::new uses DEFAULT_BUDGET
    registry::reset();
    let run = RunEnv::new();

    // 13 always-due non-critical threads — the full scheduled roster size.
    let mut mind = Mind::new();
    for i in 0..13 {
        mind.register(Box::new(BusyThread {
            id: format!("busy_{i}"),
        }));
    }

    let mut ctx = run.ctx(1_000);
    let outcomes = mind.run_due(&mut ctx);
    let ran = outcomes.iter().filter(|o| o.ran).count();
    assert_eq!(
        ran, 13,
        "with the default per-tick budget every scheduled non-critical thread must tick in one \
         pass ({ran} ran) — RED under the old default budget of 2, which starves the roster"
    );
}

// ===========================================================================
// 3. simard.thread.* telemetry present for EVERY registered thread.
// ===========================================================================

#[test]
#[serial]
fn every_registered_thread_emits_its_telemetry_series() {
    let _env = EnvGuard::clean(); // all default-ON
    registry::reset();
    let run = RunEnv::new();

    let mut mind = Mind::with_budget(64);
    register_master_gated_roster(&mut mind, run.tmp.path());

    // Roster ids that are actually enabled (default-ON ⇒ all of them).
    let ids: Vec<String> = mind
        .health()
        .into_iter()
        .filter(|h| h.enabled)
        .map(|h| h.id)
        .collect();
    assert!(
        ids.len() >= 2 + REFLECTIVE.len(),
        "the full roster is enabled and will tick: {ids:?}"
    );

    let mut ctx = run.ctx(10_000);
    let _ = mind.run_due(&mut ctx);

    let snap = registry::capture();
    for id in &ids {
        let runs = names::thread_metric_name(id, names::THREAD_SUFFIX_RUNS);
        assert!(
            snap.counter(&runs, &[]).is_some(),
            "every enabled thread must emit a real `simard.thread.{id}.runs` series after a \
             tick (no thread is telemetry-invisible): missing {runs}"
        );
    }
}

// ===========================================================================
// 4. Injected failure: dual-route + detect + exactly-one dispatch + notifiable.
// ===========================================================================

#[test]
#[serial]
fn injected_failure_dual_routes_detects_and_dispatches_exactly_once() {
    let _env = EnvGuard::clean();
    registry::reset();
    let _ = failure_sink::drain_recent(); // clear residue

    let run = RunEnv::new();
    let id = "injected_flaky";

    let mut mind = Mind::with_budget(64);
    mind.register(Box::new(FailingThread { id: id.to_string() }));

    // Drive >= MIN_RUNS_FOR_RATE ticks, advancing `now` past the per-thread
    // backoff each time so every pass actually runs (6 runs, all failing).
    for i in 0..6u64 {
        let mut ctx = run.ctx(1_000_000 + i * 3_600);
        let _ = mind.run_due(&mut ctx);
    }

    // --- Dual-route channel 1: the real per-thread failures counter. ---
    let snap = registry::capture();
    let failures = snap
        .counter(
            &names::thread_metric_name(id, names::THREAD_SUFFIX_FAILURES),
            &[],
        )
        .unwrap_or(0);
    assert!(
        failures >= 5,
        "every failing tick bumps the real failures counter (got {failures})"
    );

    // --- Dual-route channel 2: the durable, Overseer-drained diagnosis. ---
    let drained = failure_sink::drain_recent();
    assert!(
        drained
            .iter()
            .any(|d| d.cause == FailureCause::CognitiveThread),
        "a failing thread also records a durable CognitiveThread FailureDiagnosis — the \
         failure is never swallowed inside the thread"
    );

    // --- Detect off the REAL telemetry ⇒ a stable, thread-named anomaly. ---
    let registry_view = vec![ThreadHealth {
        id: id.to_string(),
        enabled: true,
        last_run_epoch: None,
        next_run_epoch: None,
        last_success: Some(false),
        consecutive_errors: 6,
        backoff_until_epoch: None,
        purpose: "injected failing cognitive thread".to_string(),
        cadence_secs: Some(300),
    }];
    let anomalies = detect_thread_anomalies(&snap, &registry_view, &[], 1_000_000 + 5 * 3_600);
    assert!(
        anomalies.iter().any(|a| a.contains(id)),
        "the Overseer detects the failing thread from its real telemetry: {anomalies:?}"
    );

    // --- Same Observe→Orient fan-out ⇒ EXACTLY ONE dispatch, notifiable. ---
    let state = ObservedState {
        anomalies,
        ..ObservedState::default()
    };
    let signals = signal::signals_from(&state);
    assert!(
        signals.iter().any(|s| matches!(s, Signal::Anomaly { .. })),
        "each anomaly lifts into a Signal::Anomaly (the notify fan-out input)"
    );
    let problems = orient(&signals, &[]);
    let health: Vec<_> = problems
        .iter()
        .filter(|p| p.kind == ProblemKind::ProcessHealth)
        .collect();
    assert_eq!(
        health.len(),
        1,
        "the injected failure drives exactly ONE deduplicated remediation dispatch: {problems:?}"
    );
    assert!(
        !health[0].summary.is_empty() && health[0].dedup_key.starts_with("anomaly:"),
        "the dispatched problem carries a notifiable summary and a stable dedup key: {:?}",
        health[0]
    );
}

// ===========================================================================
// 5. Security regressions T-S2..T-S5.
// ===========================================================================

// T-S2 — exec-boundary injection: memory/log-derived text fed into remediation
// `-c` values cannot smuggle a second argv pair or a fresh prompt line.
#[test]
fn ts2_sanitize_and_fence_neutralize_injected_control_and_delimiters() {
    let hostile = "safe value\n-c evil=1\r\n\0second line";
    let cleaned = sanitize_value(hostile);
    assert!(
        !cleaned.contains('\n') && !cleaned.contains('\r') && !cleaned.contains('\0'),
        "sanitize_value strips every control char so no newline/NUL can add a `-c` pair: {cleaned:?}"
    );
    assert!(
        cleaned.contains("safe value") && cleaned.contains("-c evil=1"),
        "printable content is preserved (only control chars are removed): {cleaned:?}"
    );

    // Fencing wraps untrusted memory as DATA and neutralises an embedded close
    // delimiter so the payload can never escape the region into instructions.
    let escape = format!("legit {UNTRUSTED_CLOSE} now I am instructions");
    let fenced = fence_untrusted(&escape);
    assert!(
        is_fenced_payload(&fenced),
        "output is a fenced untrusted payload"
    );
    assert!(
        fenced.starts_with(UNTRUSTED_OPEN) && fenced.trim_end().ends_with(UNTRUSTED_CLOSE),
        "the payload is bounded by the region delimiters"
    );
    let inner = &fenced[UNTRUSTED_OPEN.len()..fenced.len() - UNTRUSTED_CLOSE.len()];
    assert!(
        !inner.contains(UNTRUSTED_CLOSE),
        "an embedded closing delimiter is neutralised so memory text cannot end the fence early"
    );
}

// T-S3 — path-traversal / recipe-path escape: concept keys with a separator or
// `..` are rejected outright (never truncated into something traversal-shaped).
#[test]
fn ts3_validate_concept_key_rejects_traversal_and_separators() {
    for bad in ["../etc/passwd", "a/b", "a\\b", "..", "foo/..", "x/../y"] {
        assert!(
            validate_concept_key(bad).is_none(),
            "a key with a path separator or `..` must be rejected: {bad:?}"
        );
    }
    assert_eq!(
        validate_concept_key("metacog_calibration").as_deref(),
        Some("metacog_calibration"),
        "an ordinary snake_case key is accepted unchanged"
    );
}

// T-S4 — remediation-storm DoS: when the whole roster is unhealthy at once, the
// detector's per-cycle emission cap bounds the notification/dispatch pressure.
#[test]
fn ts4_anomaly_emission_is_capped_under_a_full_roster_outage() {
    const NOW: u64 = 2_000_000_000;
    let mut registry_view = Vec::new();
    let mut snap = MetricsSnapshot::empty();
    for i in 0..50 {
        let id = format!("thread_{i}");
        registry_view.push(ThreadHealth {
            id: id.clone(),
            enabled: true,
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
            consecutive_errors: 0,
            backoff_until_epoch: None,
            purpose: "stalled".to_string(),
            cadence_secs: Some(300),
        });
        // Every one is long-overdue ⇒ every one is anomalous.
        snap.gauges.push(GaugeSeries {
            name: format!("simard.thread.{id}.next_run_epoch"),
            attrs: Vec::new(),
            value: (NOW - 3_600) as i64,
        });
        snap.counters.push(CounterSeries {
            name: format!("simard.thread.{id}.runs"),
            attrs: Vec::new(),
            value: 5,
        });
    }
    let anomalies = detect_thread_anomalies(&snap, &registry_view, &[], NOW);
    assert!(
        anomalies.len() <= MAX_THREAD_ANOMALIES_PER_CYCLE,
        "a 50-thread outage must not flood remediation: {} > cap {}",
        anomalies.len(),
        MAX_THREAD_ANOMALIES_PER_CYCLE
    );
}

// T-S5 — no swallowed failures: a caught, panicking thread still dual-routes its
// failure (real failures counter + durable diagnosis) and never downs the Mind.
#[test]
#[serial]
fn ts5_panicking_thread_is_isolated_and_its_failure_is_not_swallowed() {
    registry::reset();
    let _ = failure_sink::drain_recent();
    let run = RunEnv::new();

    struct PanicThread;
    impl CognitiveThread for PanicThread {
        fn id(&self) -> &str {
            "panicky"
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
            panic!("injected panic inside a cognitive thread");
        }
        fn health(&self) -> ThreadHealth {
            ThreadHealth {
                id: "panicky".to_string(),
                enabled: true,
                last_run_epoch: None,
                next_run_epoch: None,
                last_success: None,
                consecutive_errors: 0,
                backoff_until_epoch: None,
                purpose: "panicking thread".to_string(),
                cadence_secs: Some(0),
            }
        }
    }

    let mut mind = Mind::with_budget(64);
    mind.register(Box::new(PanicThread));
    mind.register(Box::new(BusyThread {
        id: "survivor".to_string(),
    }));

    let mut ctx = run.ctx(5_000);
    // The Mind must NOT propagate the panic (isolation backstop).
    let outcomes = mind.run_due(&mut ctx);
    assert!(
        outcomes.iter().any(|o| o.success),
        "a sibling thread still succeeds after a peer panics (panic is isolated)"
    );
    assert!(
        outcomes.iter().any(|o| !o.success),
        "the panicking thread's tick is surfaced as a failed outcome, not silently dropped"
    );

    let snap = registry::capture();
    assert_eq!(
        snap.counter(
            &names::thread_metric_name("panicky", names::THREAD_SUFFIX_FAILURES),
            &[]
        ),
        Some(1),
        "a caught panic is counted as a failure — not swallowed"
    );
    assert!(
        failure_sink::drain_recent()
            .iter()
            .any(|d| d.cause == FailureCause::CognitiveThread),
        "a caught panic also records a durable diagnosis for the Overseer"
    );
}
