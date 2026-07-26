//! TDD contract (Step 7) for the deterministic thread-oversight rail (issue
//! #4786, design component C7 / requirement R5). Authored **before**
//! `overseer::thread_oversight` exists, so this module is RED until that module
//! ships:
//!
//!   * `detect_thread_anomalies(snapshot, registry, ooda_tail, now) ->
//!     Vec<String>` — a PURE, panic-free, bounded detector. It reads the
//!     per-thread telemetry series out of the metrics snapshot, the
//!     name+purpose+cadence registry (`Mind::health()`, design C5), and a
//!     bounded tail of `~/.simard/ooda.log`, and returns human-readable anomaly
//!     strings.
//!   * `MAX_THREAD_ANOMALIES_PER_CYCLE` — the per-cycle emission cap that
//!     prevents notification flooding (security SR-R5).
//!
//! It also pins the registry single-source-of-truth fields
//! `ThreadHealth { purpose, cadence_secs }` (design C3) that `Mind::health()`
//! populates.
//!
//! The detector is pure, so these tests are hermetic: no registry, no I/O, no
//! `#[serial]`. Series are hand-built with the fixed `simard.thread.<id>.<suffix>`
//! naming and an EMPTY attribute set (identity is in the name, design A3).

use crate::cognitive_threads::ThreadHealth;
use crate::overseer::thread_oversight::{MAX_THREAD_ANOMALIES_PER_CYCLE, detect_thread_anomalies};
use crate::telemetry::snapshot::{CounterSeries, GaugeSeries, MetricsSnapshot};

/// A base "now" far from the epoch so `now - N` never underflows.
const NOW: u64 = 2_000_000_000;

fn tname(id: &str, suffix: &str) -> String {
    format!("simard.thread.{id}.{suffix}")
}

fn gauge(name: String, value: i64) -> GaugeSeries {
    GaugeSeries {
        name,
        attrs: Vec::new(),
        value,
    }
}

fn counter(name: String, value: u64) -> CounterSeries {
    CounterSeries {
        name,
        attrs: Vec::new(),
        value,
    }
}

/// A registry entry (as produced by `Mind::health()`), carrying the
/// single-source-of-truth purpose + cadence (design C3/C5).
fn reg_entry(id: &str, cadence_secs: u64) -> ThreadHealth {
    ThreadHealth {
        id: id.to_string(),
        enabled: true,
        last_run_epoch: None,
        next_run_epoch: None,
        last_success: None,
        consecutive_errors: 0,
        backoff_until_epoch: None,
        purpose: format!("{id} purpose"),
        cadence_secs: Some(cadence_secs),
    }
}

#[test]
fn healthy_on_cadence_thread_yields_no_anomaly() {
    let cadence = 300_u64;
    let registry = vec![reg_entry("ooda", cadence)];

    let mut snap = MetricsSnapshot::empty();
    snap.gauges.push(gauge(
        tname("ooda", "next_run_epoch"),
        (NOW + cadence) as i64,
    ));
    snap.gauges
        .push(gauge(tname("ooda", "last_run_epoch"), (NOW - 10) as i64));
    snap.counters.push(counter(tname("ooda", "runs"), 100));
    snap.counters.push(counter(tname("ooda", "successes"), 100));
    snap.counters.push(counter(tname("ooda", "failures"), 0));

    let anomalies = detect_thread_anomalies(&snap, &registry, &[], NOW);
    assert!(
        anomalies.is_empty(),
        "a fresh, succeeding, on-cadence thread is not anomalous: {anomalies:?}"
    );
}

#[test]
fn stalled_next_run_yields_an_anomaly_naming_the_thread() {
    let cadence = 300_u64;
    let registry = vec![reg_entry("maintenance", cadence)];

    let mut snap = MetricsSnapshot::empty();
    // Next run was due an hour ago — ~12 cadences overdue: unambiguously stalled.
    snap.gauges.push(gauge(
        tname("maintenance", "next_run_epoch"),
        (NOW - 3600) as i64,
    ));
    snap.gauges.push(gauge(
        tname("maintenance", "last_run_epoch"),
        (NOW - 3900) as i64,
    ));
    snap.counters.push(counter(tname("maintenance", "runs"), 5));

    let anomalies = detect_thread_anomalies(&snap, &registry, &[], NOW);
    assert!(
        anomalies.iter().any(|a| a.contains("maintenance")),
        "a thread whose next_run_epoch is long past is anomalous and named: {anomalies:?}"
    );
    assert!(
        anomalies.len() <= MAX_THREAD_ANOMALIES_PER_CYCLE,
        "output stays within the per-cycle cap"
    );
}

#[test]
fn high_failure_rate_yields_an_anomaly() {
    let cadence = 300_u64;
    let registry = vec![reg_entry("engineer_log", cadence)];

    let mut snap = MetricsSnapshot::empty();
    // On cadence & recently run, but 8 of 10 attempts failed.
    snap.gauges.push(gauge(
        tname("engineer_log", "next_run_epoch"),
        (NOW + cadence) as i64,
    ));
    snap.gauges.push(gauge(
        tname("engineer_log", "last_run_epoch"),
        (NOW - 10) as i64,
    ));
    snap.counters
        .push(counter(tname("engineer_log", "runs"), 10));
    snap.counters
        .push(counter(tname("engineer_log", "failures"), 8));

    let anomalies = detect_thread_anomalies(&snap, &registry, &[], NOW);
    assert!(
        anomalies.iter().any(|a| a.contains("engineer_log")),
        "a thread failing the majority of its runs is anomalous: {anomalies:?}"
    );
}

#[test]
fn ooda_log_error_line_yields_an_anomaly() {
    let registry = vec![reg_entry("ooda", 300)];

    let mut snap = MetricsSnapshot::empty();
    snap.gauges
        .push(gauge(tname("ooda", "next_run_epoch"), (NOW + 300) as i64));
    snap.gauges
        .push(gauge(tname("ooda", "last_run_epoch"), (NOW - 10) as i64));

    let tail = vec![
        "2026-07-26T19:00:00Z INFO ooda cycle complete".to_string(),
        "2026-07-26T19:05:00Z ERROR ooda: decision parse failed".to_string(),
    ];

    let anomalies = detect_thread_anomalies(&snap, &registry, &tail, NOW);
    assert!(
        !anomalies.is_empty(),
        "an ERROR line in the ooda.log tail must surface an anomaly"
    );
}

#[test]
fn ooda_log_error_substrings_do_not_false_positive() {
    // `error` must match only as a whole word: `0 errors` (trailing word char)
    // and `error-free` (compound) are NOT error lines and must not surface an
    // anomaly on their own.
    let registry = vec![reg_entry("ooda", 300)];
    let mut snap = MetricsSnapshot::empty();
    snap.gauges
        .push(gauge(tname("ooda", "next_run_epoch"), (NOW + 300) as i64));
    snap.gauges
        .push(gauge(tname("ooda", "last_run_epoch"), (NOW - 10) as i64));

    let tail = vec![
        "2026-07-26T19:00:00Z INFO ooda cycle complete with 0 errors".to_string(),
        "2026-07-26T19:05:00Z INFO ooda run was error-free".to_string(),
    ];

    let anomalies = detect_thread_anomalies(&snap, &registry, &tail, NOW);
    assert!(
        anomalies.is_empty(),
        "`0 errors` / `error-free` are not error lines: {anomalies:?}"
    );
}

#[test]
fn ooda_log_error_tokens_with_punctuation_still_match() {
    // Genuine error-level markers embedded in punctuation (`[error]`, `error:`)
    // must still surface an anomaly.
    let registry = vec![reg_entry("ooda", 300)];
    let mut snap = MetricsSnapshot::empty();
    snap.gauges
        .push(gauge(tname("ooda", "next_run_epoch"), (NOW + 300) as i64));
    snap.gauges
        .push(gauge(tname("ooda", "last_run_epoch"), (NOW - 10) as i64));

    let tail = vec!["2026-07-26T19:05:00Z [error] ooda: decision parse failed".to_string()];

    let anomalies = detect_thread_anomalies(&snap, &registry, &tail, NOW);
    assert!(
        !anomalies.is_empty(),
        "punctuation-delimited `[error]` must surface an anomaly"
    );
}

#[test]
fn malformed_input_degrades_to_bounded_result_without_panic() {
    // Empty registry + empty snapshot + junk tail must not panic (SR-R1) and
    // must stay within the cap (SR-R5).
    let snap = MetricsSnapshot::empty();
    let tail = vec![
        "\u{feff}garbled partial line with no timestamp".to_string(),
        String::new(),
        "ERROR".to_string(),
    ];

    let anomalies = detect_thread_anomalies(&snap, &[], &tail, 0);
    assert!(
        anomalies.len() <= MAX_THREAD_ANOMALIES_PER_CYCLE,
        "even on malformed input the detector returns a bounded, panic-free result"
    );
}

#[test]
fn anomaly_emissions_are_capped_per_cycle() {
    let cadence = 300_u64;

    // 50 stalled threads — every one is anomalous, but the rail must cap output.
    let mut registry = Vec::new();
    let mut snap = MetricsSnapshot::empty();
    for i in 0..50 {
        let id = format!("thread_{i}");
        registry.push(reg_entry(&id, cadence));
        snap.gauges
            .push(gauge(tname(&id, "next_run_epoch"), (NOW - 3600) as i64));
        snap.gauges
            .push(gauge(tname(&id, "last_run_epoch"), (NOW - 3900) as i64));
    }

    let anomalies = detect_thread_anomalies(&snap, &registry, &[], NOW);
    assert!(
        anomalies.len() <= MAX_THREAD_ANOMALIES_PER_CYCLE,
        "anomaly output is bounded to avoid notification flooding: {} > {}",
        anomalies.len(),
        MAX_THREAD_ANOMALIES_PER_CYCLE
    );
    const {
        assert!(
            MAX_THREAD_ANOMALIES_PER_CYCLE < 50,
            "the cap must actually bound a 50-thread flood"
        )
    };
}
