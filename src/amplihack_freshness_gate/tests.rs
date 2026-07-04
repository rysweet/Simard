//! TDD suite for the amplihack freshness gate.
//!
//! Every test is hermetic: a `tempfile` state root, injected fake updater +
//! clock + metric sink, and **no** process-global env mutation, real network,
//! subprocess, or wall clock. That keeps them safe under cargo's parallel runner
//! and independent of the host's `amplihack` install.
//!
//! Coverage maps directly to the operator's required scenarios:
//! - the gate RUNS an update before a spawn when the TTL is expired;
//! - it SKIPS when a success is within the TTL (boundary is inclusive);
//! - concurrent spawns SERIALIZE so the updater runs exactly once;
//! - an update FAILURE surfaces an explicit error + metric and (default) still
//!   spawns, while `SIMARD_REQUIRE_FRESH_AMPLIHACK=1` blocks with an explicit
//!   error;
//! - the gate is disabled via `SIMARD_ENGINEER_AMPLIHACK_UPDATE=0`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::gate::{
    parse_enabled, parse_require_fresh, parse_ttl, read_last_success, write_last_success,
};
use super::{
    AmplihackUpdater, DEFAULT_TTL_SECS, FAILURE_METRIC, GateClock, GateConfig, GateOutcome,
    MetricSink, UPDATE_LOCK_FILENAME, UPDATE_STATE_FILENAME, run_freshness_gate,
};

// ─────────────────────────── fakes ─────────────────────────────────────────

/// Counts invocations and returns a canned result, so a test can assert both
/// "ran once" and success/failure behaviour.
struct FakeUpdater {
    calls: Arc<AtomicUsize>,
    result: Result<(), String>,
}

impl FakeUpdater {
    fn ok(calls: Arc<AtomicUsize>) -> Self {
        Self {
            calls,
            result: Ok(()),
        }
    }

    fn failing(calls: Arc<AtomicUsize>) -> Self {
        Self {
            calls,
            result: Err("simulated build/network failure".to_string()),
        }
    }
}

impl AmplihackUpdater for FakeUpdater {
    fn run_update(&self) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

/// Fixed clock so TTL arithmetic is deterministic.
struct FixedClock {
    now: i64,
}

impl GateClock for FixedClock {
    fn now_epoch_secs(&self) -> i64 {
        self.now
    }
}

/// Captures metric records instead of writing the global `metrics.jsonl`.
#[derive(Default)]
struct FakeMetrics {
    records: Mutex<Vec<(String, f64, String)>>,
}

impl MetricSink for FakeMetrics {
    fn record(&self, name: &str, value: f64, context: &str) {
        self.records.lock().expect("metrics lock").push((
            name.to_string(),
            value,
            context.to_string(),
        ));
    }
}

impl FakeMetrics {
    fn records(&self) -> Vec<(String, f64, String)> {
        self.records.lock().expect("metrics lock").clone()
    }
}

// ─────────────────────────── behaviour: run / skip ─────────────────────────

#[test]
fn runs_update_when_no_prior_success() {
    let dir = tempfile::tempdir().expect("tempdir");
    let calls = Arc::new(AtomicUsize::new(0));
    let updater = FakeUpdater::ok(Arc::clone(&calls));
    let clock = FixedClock { now: 1_000 };
    let metrics = FakeMetrics::default();
    let config = GateConfig::default();

    let outcome = run_freshness_gate(dir.path(), &config, &updater, &clock, &metrics);

    assert_eq!(outcome, GateOutcome::Ran);
    assert!(outcome.should_spawn());
    assert_eq!(calls.load(Ordering::SeqCst), 1, "update must run once");
    assert_eq!(
        read_last_success(dir.path()),
        Some(1_000),
        "a successful run records the timestamp",
    );
    assert!(
        metrics.records().is_empty(),
        "success records no failure metric"
    );
}

#[test]
fn skips_update_within_ttl() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_last_success(dir.path(), 1_000).expect("seed timestamp");

    let calls = Arc::new(AtomicUsize::new(0));
    let updater = FakeUpdater::ok(Arc::clone(&calls));
    // now - last_success = 100 <= ttl 300 ⇒ fresh.
    let clock = FixedClock { now: 1_100 };
    let metrics = FakeMetrics::default();
    let config = GateConfig::default();

    let outcome = run_freshness_gate(dir.path(), &config, &updater, &clock, &metrics);

    assert_eq!(outcome, GateOutcome::SkippedFresh);
    assert!(outcome.should_spawn());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "fresh install skips the update"
    );
    assert!(metrics.records().is_empty());
}

#[test]
fn runs_update_when_ttl_expired() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_last_success(dir.path(), 1_000).expect("seed timestamp");

    let calls = Arc::new(AtomicUsize::new(0));
    let updater = FakeUpdater::ok(Arc::clone(&calls));
    // age = 301 > ttl 300 ⇒ stale.
    let clock = FixedClock { now: 1_301 };
    let metrics = FakeMetrics::default();
    let config = GateConfig::default();

    let outcome = run_freshness_gate(dir.path(), &config, &updater, &clock, &metrics);

    assert_eq!(outcome, GateOutcome::Ran);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        read_last_success(dir.path()),
        Some(1_301),
        "a fresh run advances the timestamp",
    );
}

#[test]
fn ttl_boundary_is_inclusive_skip() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_last_success(dir.path(), 1_000).expect("seed timestamp");

    let calls = Arc::new(AtomicUsize::new(0));
    let updater = FakeUpdater::ok(Arc::clone(&calls));
    // age == ttl exactly ⇒ still fresh (inclusive).
    let clock = FixedClock {
        now: 1_000 + DEFAULT_TTL_SECS,
    };
    let metrics = FakeMetrics::default();
    let config = GateConfig::default();

    let outcome = run_freshness_gate(dir.path(), &config, &updater, &clock, &metrics);

    assert_eq!(outcome, GateOutcome::SkippedFresh);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn future_timestamp_is_not_treated_as_fresh() {
    // A last_success in the future (clock skew / corruption) must not wedge the
    // gate into skipping forever — it runs.
    let dir = tempfile::tempdir().expect("tempdir");
    write_last_success(dir.path(), 5_000).expect("seed timestamp");

    let calls = Arc::new(AtomicUsize::new(0));
    let updater = FakeUpdater::ok(Arc::clone(&calls));
    let clock = FixedClock { now: 1_000 };
    let metrics = FakeMetrics::default();
    let config = GateConfig::default();

    let outcome = run_freshness_gate(dir.path(), &config, &updater, &clock, &metrics);

    assert_eq!(outcome, GateOutcome::Ran);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// ─────────────────────────── behaviour: serialize + dedup ──────────────────

#[test]
fn concurrent_spawns_serialize_and_update_runs_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let calls = Arc::new(AtomicUsize::new(0));
    let outcomes = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::new();
    for _ in 0..8 {
        let root = root.clone();
        let calls = Arc::clone(&calls);
        let outcomes = Arc::clone(&outcomes);
        handles.push(std::thread::spawn(move || {
            // Each spawner brings its own deps but shares the call counter and
            // the on-disk lock + timestamp under `root`. A fixed clock means the
            // first writer's timestamp is immediately "fresh" for the rest.
            let updater = FakeUpdater::ok(calls);
            let clock = FixedClock { now: 2_000 };
            let metrics = FakeMetrics::default();
            let config = GateConfig::default();
            let outcome = run_freshness_gate(&root, &config, &updater, &clock, &metrics);
            outcomes.lock().expect("outcomes lock").push(outcome);
        }));
    }
    for h in handles {
        h.join().expect("join spawner");
    }

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the flock + TTL must collapse a burst to exactly one update",
    );

    let outcomes = outcomes.lock().expect("outcomes lock").clone();
    assert_eq!(outcomes.len(), 8);
    let ran = outcomes.iter().filter(|o| **o == GateOutcome::Ran).count();
    let skipped = outcomes
        .iter()
        .filter(|o| **o == GateOutcome::SkippedFresh)
        .count();
    assert_eq!(ran, 1, "exactly one spawner runs the update");
    assert_eq!(
        skipped, 7,
        "the rest skip on the just-written fresh timestamp"
    );
}

#[test]
fn lock_is_released_so_a_later_stale_gate_can_run_again() {
    // Two sequential evaluations: the first runs (no prior success), the second
    // is stale again and must be able to run — proving the lock never stranded.
    let dir = tempfile::tempdir().expect("tempdir");
    let calls = Arc::new(AtomicUsize::new(0));
    let metrics = FakeMetrics::default();
    let config = GateConfig::default();

    let first = run_freshness_gate(
        dir.path(),
        &config,
        &FakeUpdater::ok(Arc::clone(&calls)),
        &FixedClock { now: 1_000 },
        &metrics,
    );
    assert_eq!(first, GateOutcome::Ran);

    // Advance well past the TTL so the second evaluation is stale again.
    let second = run_freshness_gate(
        dir.path(),
        &config,
        &FakeUpdater::ok(Arc::clone(&calls)),
        &FixedClock {
            now: 1_000 + DEFAULT_TTL_SECS + 1,
        },
        &metrics,
    );
    assert_eq!(second, GateOutcome::Ran);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

// ─────────────────────────── behaviour: failure surfacing ──────────────────

#[test]
fn update_failure_default_surfaces_metric_and_still_spawns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let calls = Arc::new(AtomicUsize::new(0));
    let updater = FakeUpdater::failing(Arc::clone(&calls));
    let clock = FixedClock { now: 1_000 };
    let metrics = FakeMetrics::default();
    let config = GateConfig::default(); // require_fresh = false

    let outcome = run_freshness_gate(dir.path(), &config, &updater, &clock, &metrics);

    assert_eq!(outcome, GateOutcome::Failed);
    assert!(
        outcome.should_spawn(),
        "default proceeds on last-known-good"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let records = metrics.records();
    assert_eq!(records.len(), 1, "one failure metric is recorded");
    let (name, value, context) = &records[0];
    assert_eq!(name, FAILURE_METRIC);
    assert_eq!(*value, 1.0);
    assert!(
        context.contains("last-known-good"),
        "context names the last-known-good decision: {context}",
    );

    assert_eq!(
        read_last_success(dir.path()),
        None,
        "a failed update must not advance the timestamp",
    );
}

#[test]
fn update_failure_strict_mode_blocks_with_explicit_error_and_metric() {
    let dir = tempfile::tempdir().expect("tempdir");
    let calls = Arc::new(AtomicUsize::new(0));
    let updater = FakeUpdater::failing(Arc::clone(&calls));
    let clock = FixedClock { now: 1_000 };
    let metrics = FakeMetrics::default();
    let config = GateConfig {
        require_fresh: true,
        ..GateConfig::default()
    };

    let outcome = run_freshness_gate(dir.path(), &config, &updater, &clock, &metrics);

    assert_eq!(outcome, GateOutcome::Blocked);
    assert!(!outcome.should_spawn(), "strict mode refuses the spawn");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let records = metrics.records();
    assert_eq!(records.len(), 1);
    let (name, _value, context) = &records[0];
    assert_eq!(name, FAILURE_METRIC);
    assert!(
        context.contains("blocked"),
        "context names the blocked decision: {context}",
    );
}

// ─────────────────────────── behaviour: disabled ───────────────────────────

#[test]
fn disabled_gate_skips_everything() {
    let dir = tempfile::tempdir().expect("tempdir");
    let calls = Arc::new(AtomicUsize::new(0));
    let updater = FakeUpdater::ok(Arc::clone(&calls));
    let clock = FixedClock { now: 1_000 };
    let metrics = FakeMetrics::default();
    let config = GateConfig {
        enabled: false,
        ..GateConfig::default()
    };

    let outcome = run_freshness_gate(dir.path(), &config, &updater, &clock, &metrics);

    assert_eq!(outcome, GateOutcome::Disabled);
    assert!(
        outcome.should_spawn(),
        "a disabled gate never blocks a spawn"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no update runs");
    assert!(metrics.records().is_empty());
    assert!(
        !dir.path().join(UPDATE_LOCK_FILENAME).exists(),
        "disabled gate touches no lock file",
    );
    assert!(
        !dir.path().join(UPDATE_STATE_FILENAME).exists(),
        "disabled gate touches no state file",
    );
}

// ─────────────────────────── outcome mapping ───────────────────────────────

#[test]
fn only_blocked_stops_the_spawn() {
    assert!(GateOutcome::Ran.should_spawn());
    assert!(GateOutcome::SkippedFresh.should_spawn());
    assert!(GateOutcome::Failed.should_spawn());
    assert!(GateOutcome::Disabled.should_spawn());
    assert!(!GateOutcome::Blocked.should_spawn());
}

#[test]
fn outcome_tokens_match_the_contract() {
    assert_eq!(GateOutcome::Ran.as_str(), "ran");
    assert_eq!(GateOutcome::SkippedFresh.as_str(), "skipped-fresh");
    assert_eq!(GateOutcome::Failed.as_str(), "failed");
    assert_eq!(GateOutcome::Blocked.as_str(), "blocked");
    assert_eq!(GateOutcome::Disabled.as_str(), "disabled");
}

// ─────────────────────────── durable state schema ──────────────────────────

#[test]
fn state_file_uses_the_documented_single_field_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_last_success(dir.path(), 1_751_645_000).expect("write state");

    let raw = std::fs::read_to_string(dir.path().join(UPDATE_STATE_FILENAME)).expect("read state");
    assert!(
        raw.contains("last_success_epoch_secs"),
        "on-disk schema must expose last_success_epoch_secs: {raw}",
    );
    assert_eq!(read_last_success(dir.path()), Some(1_751_645_000));
}

#[test]
fn absent_or_corrupt_state_reads_as_no_prior_success() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(read_last_success(dir.path()), None, "absent ⇒ None");

    std::fs::write(dir.path().join(UPDATE_STATE_FILENAME), b"{ not json").expect("write junk");
    assert_eq!(read_last_success(dir.path()), None, "corrupt ⇒ None");
}

// ─────────────────────────── config parsing (pure, no env) ─────────────────

#[test]
fn parse_enabled_defaults_on_and_zero_disables() {
    assert!(
        parse_enabled(None),
        "unset defaults ON per operator directive"
    );
    assert!(parse_enabled(Some("1")));
    assert!(parse_enabled(Some("anything")));
    assert!(!parse_enabled(Some("0")));
    assert!(!parse_enabled(Some(" 0 ")), "whitespace is trimmed");
    assert!(!parse_enabled(Some("false")));
    assert!(!parse_enabled(Some("OFF")));
}

#[test]
fn parse_require_fresh_off_by_default_and_one_enables() {
    assert!(!parse_require_fresh(None), "strict mode is off by default");
    assert!(!parse_require_fresh(Some("0")));
    assert!(parse_require_fresh(Some("1")));
    assert!(parse_require_fresh(Some(" 1 ")));
    assert!(parse_require_fresh(Some("true")));
    assert!(!parse_require_fresh(Some("nonsense")));
}

#[test]
fn parse_ttl_default_and_overrides() {
    assert_eq!(parse_ttl(None), DEFAULT_TTL_SECS);
    assert_eq!(parse_ttl(Some("60")), 60);
    assert_eq!(parse_ttl(Some(" 90 ")), 90);
    assert_eq!(parse_ttl(Some("0")), 0, "zero TTL means always-run");
    assert_eq!(
        parse_ttl(Some("bad")),
        DEFAULT_TTL_SECS,
        "invalid ⇒ default"
    );
    assert_eq!(
        parse_ttl(Some("-5")),
        DEFAULT_TTL_SECS,
        "negative ⇒ default"
    );
}

#[test]
fn zero_ttl_always_runs_the_update() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_last_success(dir.path(), 1_000).expect("seed timestamp");

    let calls = Arc::new(AtomicUsize::new(0));
    let updater = FakeUpdater::ok(Arc::clone(&calls));
    // age == 0 but ttl == 0 ⇒ 0 <= 0 is inclusive, so this is the one skip case
    // at exactly-now; advance by 1s so age(1) > ttl(0) and it runs.
    let clock = FixedClock { now: 1_001 };
    let metrics = FakeMetrics::default();
    let config = GateConfig {
        ttl_secs: 0,
        ..GateConfig::default()
    };

    let outcome = run_freshness_gate(dir.path(), &config, &updater, &clock, &metrics);
    assert_eq!(outcome, GateOutcome::Ran);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
