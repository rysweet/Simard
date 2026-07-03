//! Unit/integration tests for the unified telemetry facade (issue #2528).
//!
//! These pin the documented contract in `docs/reference/telemetry-metrics.md`:
//! the `counter_add`/`gauge_set`/`histogram_record` facade, the bounded
//! in-process registry (attribute normalization, per-key cardinality bound with
//! `other` overflow), the `MetricsSnapshot` serde shape + accessors, and the
//! degrade-safe atomic on-disk snapshot writer/reader.
//!
//! The registry is process-global, so every test that touches it is
//! `#[serial(telemetry_registry)]` and calls `reset()` first.

use serial_test::serial;

use simard::telemetry;
use simard::telemetry::names;
use simard::telemetry::registry::{MAX_ATTR_VALUE_LEN, MAX_VALUES_PER_KEY};
use simard::telemetry::snapshot::{self, MetricsSnapshot, SCHEMA_VERSION};

// ── metric catalog ───────────────────────────────────────────────────────────

#[test]
fn metric_names_are_dotted_and_stable() {
    // A representative slice of the catalog; these strings are a public
    // contract other tooling (dashboards, alerts) keys off, so pin them.
    assert_eq!(names::DISTILL_RUNS, "simard.distill.runs");
    assert_eq!(names::DISTILL_FACTS, "simard.distill.facts");
    assert_eq!(names::BRAIN_DECISION, "simard.brain.decision");
    assert_eq!(
        names::BRAIN_LADDER_EXHAUSTED,
        "simard.brain.ladder_exhausted"
    );
    assert_eq!(names::ENGINEER_ACTIVE, "simard.engineer.active");
    assert_eq!(
        names::DAEMON_CYCLE_DURATION_SECONDS,
        "simard.daemon.cycle_duration_seconds"
    );
    assert_eq!(names::MEMORY_NODES, "simard.memory.nodes");
    assert_eq!(names::MEMORY_EDGES, "simard.memory.edges");
    assert_eq!(names::LLM_TOKENS, "simard.llm.tokens");
    assert_eq!(names::GOAL_ACTIVE, "simard.goal.active");

    for name in [
        names::DISTILL_RUNS,
        names::BRAIN_DECISION,
        names::ENGINEER_SPAWNED,
        names::DAEMON_CYCLE,
        names::MEMORY_NODES,
        names::LLM_COST_USD,
        names::GOAL_PROGRESS,
    ] {
        assert!(name.starts_with("simard."), "not dotted: {name}");
    }
}

// ── facade behavior ──────────────────────────────────────────────────────────

#[test]
#[serial(telemetry_registry)]
fn counter_add_accumulates_per_attribute_set() {
    telemetry::reset();
    telemetry::counter_add(names::DISTILL_RUNS, 1, &[(names::ATTR_RESULT, "ok")]);
    telemetry::counter_add(names::DISTILL_RUNS, 2, &[(names::ATTR_RESULT, "ok")]);
    telemetry::counter_add(
        names::DISTILL_RUNS,
        5,
        &[(names::ATTR_RESULT, "parse_fail")],
    );

    let snap = telemetry::capture();
    assert_eq!(
        snap.counter(names::DISTILL_RUNS, &[(names::ATTR_RESULT, "ok")]),
        Some(3)
    );
    assert_eq!(
        snap.counter(names::DISTILL_RUNS, &[(names::ATTR_RESULT, "parse_fail")]),
        Some(5)
    );
    // Distinct attribute values are distinct series.
    assert_eq!(
        snap.counters
            .iter()
            .filter(|c| c.name == names::DISTILL_RUNS)
            .count(),
        2
    );
}

#[test]
#[serial(telemetry_registry)]
fn gauge_set_is_last_write_wins() {
    telemetry::reset();
    telemetry::gauge_set(names::ENGINEER_ACTIVE, 3, &[]);
    telemetry::gauge_set(names::ENGINEER_ACTIVE, 1, &[]);
    telemetry::gauge_set(names::ENGINEER_ACTIVE, 2, &[]);

    let snap = telemetry::capture();
    assert_eq!(snap.gauge(names::ENGINEER_ACTIVE, &[]), Some(2));
}

#[test]
#[serial(telemetry_registry)]
fn histogram_record_tracks_count_sum_and_buckets() {
    telemetry::reset();
    for v in [0.4_f64, 1.5, 7.0, 45.0] {
        telemetry::histogram_record(names::DAEMON_CYCLE_DURATION_SECONDS, v, &[]);
    }

    let snap = telemetry::capture();
    let h = snap
        .histogram(names::DAEMON_CYCLE_DURATION_SECONDS, &[])
        .expect("histogram series present");
    assert_eq!(h.count, 4);
    assert!((h.sum - 53.9).abs() < 1e-9, "sum was {}", h.sum);
    // Cumulative buckets are monotonic and the last bucket covers every
    // observation at or below its boundary.
    let mut prev = 0;
    for b in &h.buckets {
        assert!(b.count >= prev, "buckets must be cumulative/monotonic");
        prev = b.count;
    }
    assert!(prev <= h.count);
}

#[test]
#[serial(telemetry_registry)]
fn attribute_order_does_not_change_the_series() {
    telemetry::reset();
    telemetry::counter_add(
        names::LLM_TOKENS,
        10,
        &[(names::ATTR_DIR, "in"), (names::ATTR_CACHED, "false")],
    );
    telemetry::counter_add(
        names::LLM_TOKENS,
        5,
        &[(names::ATTR_CACHED, "false"), (names::ATTR_DIR, "in")],
    );

    let snap = telemetry::capture();
    // Same key regardless of the order the attributes were supplied.
    assert_eq!(
        snap.counter(
            names::LLM_TOKENS,
            &[(names::ATTR_DIR, "in"), (names::ATTR_CACHED, "false")]
        ),
        Some(15)
    );
    assert_eq!(
        snap.counters
            .iter()
            .filter(|c| c.name == names::LLM_TOKENS)
            .count(),
        1
    );
}

// ── bounds: cardinality, length, control chars ───────────────────────────────

#[test]
#[serial(telemetry_registry)]
fn unexpected_attr_values_fold_into_other_and_bump_overflow() {
    telemetry::reset();
    let metric = "simard.test.cardinality";
    let extra = 4;
    let total = MAX_VALUES_PER_KEY + extra;
    for i in 0..total {
        telemetry::counter_add(metric, 1, &[("k", &format!("v{i}"))]);
    }

    let snap = telemetry::capture();
    // The overflow values all collapse into a single `other` series.
    assert_eq!(
        snap.counter(metric, &[("k", names::OTHER_BUCKET)]),
        Some(extra as u64),
        "extra distinct values should accumulate in the `other` bucket"
    );
    // Distinct retained series is capped: MAX_VALUES_PER_KEY real + one `other`.
    assert_eq!(
        snap.counters.iter().filter(|c| c.name == metric).count(),
        MAX_VALUES_PER_KEY + 1
    );
    assert!(
        telemetry::overflow_count() >= extra as u64,
        "overflow counter should record folded values"
    );
    assert!(snap.overflow_series >= extra as u64);
}

#[test]
#[serial(telemetry_registry)]
fn attribute_values_are_length_capped() {
    telemetry::reset();
    let long = "x".repeat(MAX_ATTR_VALUE_LEN * 3);
    telemetry::counter_add("simard.test.len", 1, &[("k", &long)]);

    let snap = telemetry::capture();
    let series = snap
        .counters
        .iter()
        .find(|c| c.name == "simard.test.len")
        .expect("series present");
    let (_, value) = &series.attrs[0];
    assert_eq!(
        value.chars().count(),
        MAX_ATTR_VALUE_LEN,
        "over-long attribute value must be truncated to the cap"
    );
}

#[test]
#[serial(telemetry_registry)]
fn control_characters_are_stripped_from_attributes() {
    telemetry::reset();
    telemetry::counter_add("simard.test.ctrl", 1, &[("k", "a\nb\tc")]);

    let snap = telemetry::capture();
    let series = snap
        .counters
        .iter()
        .find(|c| c.name == "simard.test.ctrl")
        .expect("series present");
    let (_, value) = &series.attrs[0];
    assert!(
        !value.chars().any(|c| c.is_control()),
        "control characters must not survive normalization: {value:?}"
    );
    assert_eq!(value, "a b c");
}

// ── capture reflects the live registry ───────────────────────────────────────

#[test]
#[serial(telemetry_registry)]
fn capture_reflects_all_instrument_kinds() {
    telemetry::reset();
    telemetry::counter_add(names::BRAIN_ESCALATIONS, 2, &[]);
    telemetry::gauge_set(names::GOAL_ACTIVE, 7, &[]);
    telemetry::histogram_record(names::DAEMON_CYCLE_DURATION_SECONDS, 3.0, &[]);

    let snap = telemetry::capture();
    assert_eq!(snap.schema_version, SCHEMA_VERSION);
    assert!(!snap.captured_at.is_empty());
    assert_eq!(snap.counter(names::BRAIN_ESCALATIONS, &[]), Some(2));
    assert_eq!(snap.gauge(names::GOAL_ACTIVE, &[]), Some(7));
    assert_eq!(
        snap.histogram(names::DAEMON_CYCLE_DURATION_SECONDS, &[])
            .map(|h| h.count),
        Some(1)
    );
}

// ── snapshot serde + on-disk IO ──────────────────────────────────────────────

#[test]
#[serial(telemetry_registry)]
fn snapshot_json_round_trips_and_preserves_schema_version() {
    telemetry::reset();
    telemetry::counter_add(names::DISTILL_FACTS, 9, &[]);
    let original = telemetry::capture();

    let json = serde_json::to_string(&original).expect("serialize");
    assert!(json.contains("\"schema_version\""));
    let parsed: MetricsSnapshot = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(parsed.schema_version, SCHEMA_VERSION);
    assert_eq!(parsed.counter(names::DISTILL_FACTS, &[]), Some(9));
    assert_eq!(parsed, original);
}

#[test]
#[serial(telemetry_registry)]
fn write_atomic_then_read_round_trips() {
    telemetry::reset();
    telemetry::counter_add(names::DAEMON_CYCLE, 4, &[]);
    let snap = telemetry::capture();

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("telemetry").join("metrics_snapshot.json");
    snapshot::write_atomic(&path, &snap).expect("write");

    let read_back = snapshot::read(&path).expect("read");
    assert_eq!(read_back.counter(names::DAEMON_CYCLE, &[]), Some(4));
    assert_eq!(read_back, snap);
}

#[test]
fn read_missing_snapshot_is_none_not_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("does-not-exist.json");
    assert!(snapshot::read(&path).is_none());
}

#[test]
fn read_corrupt_snapshot_is_none_not_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("corrupt.json");
    std::fs::write(&path, b"{ this is not valid json ]]").expect("write");
    assert!(snapshot::read(&path).is_none());
}

#[cfg(unix)]
#[test]
#[serial(telemetry_registry)]
fn snapshot_file_is_written_private_0600() {
    use std::os::unix::fs::PermissionsExt;

    telemetry::reset();
    let snap = telemetry::capture();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("telemetry").join("metrics_snapshot.json");
    snapshot::write_atomic(&path, &snap).expect("write");

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "snapshot must not be world-readable, got {mode:o}"
    );
}
