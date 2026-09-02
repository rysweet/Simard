//! Deterministic cognitive-thread oversight rail (issue #4786, requirement R5).
//!
//! A thin, PURE, panic-free detector the acting Overseer runs each Observe pass.
//! It reasons over three read-only inputs and returns human-readable anomaly
//! strings that flow into `ObservedState.anomalies` → `Signal::Anomaly` (the
//! SAME fan-out every other observation uses — no new brittle scrape path):
//!
//!   1. the per-thread telemetry series from `metrics_snapshot.json`
//!      (`simard.thread.<id>.*`, written by the instrumented telemetry seam),
//!   2. the single-source-of-truth registry (`Mind::health()`), carrying each
//!      thread's stable id, ORIGINAL PURPOSE, and expected cadence, and
//!   3. a bounded tail of `<state_root>/ooda.log` (the daemon's own log — NOT
//!      journald), scanned for recent ERROR lines.
//!
//! The rail is deliberately THIN and deterministic (the agentic health-review
//! recipe owns the deep reasoning): it only surfaces unambiguous "this thread is
//! stalled / failing / erroring" signals. Output is CAPPED
//! ([`MAX_THREAD_ANOMALIES_PER_CYCLE`]) so a broad outage can never flood the
//! notification path, and it never panics on malformed input (SR-R1).
//!
//! Each anomaly string is STABLE per (thread, condition) across Observe passes:
//! it never embeds a live, per-cycle-varying magnitude (seconds-overdue,
//! failure counts, log excerpts). A volatile string would make the derived
//! `problem.summary` / `dedup_key` differ every cycle, defeating the Overseer's
//! `recipe_dedup_key`, recurrence-recall, and observation-write-back dedup gates
//! — re-launching an investigation (and re-recording an episode) every single
//! cycle for one persistently unhealthy thread. The live magnitudes are
//! recoverable by the investigation from telemetry / `ooda.log` directly.

use std::path::Path;

use crate::cognitive_threads::ThreadHealth;
use crate::telemetry::names;
use crate::telemetry::names::thread_metric_name as series_name;
use crate::telemetry::snapshot::MetricsSnapshot;

/// Per-cycle cap on emitted thread anomalies — bounds notification pressure when
/// many threads are simultaneously unhealthy (security SR-R5). Deliberately well
/// under the ~15-thread roster so a total outage still yields a bounded report.
pub const MAX_THREAD_ANOMALIES_PER_CYCLE: usize = 8;

/// A thread is "stalled" only once its next scheduled run is overdue by more
/// than this many cadences — generous enough that a merely-due thread (next run
/// just passed) is never flagged, only a genuinely wedged one.
const STALL_GRACE_CADENCES: u64 = 2;

/// Floor on the stall grace (seconds), so a zero/near-zero cadence thread is not
/// flagged the instant its next run passes.
const MIN_STALL_GRACE_SECS: u64 = 60;

/// Minimum lifetime run count before the failure-rate check fires — avoids
/// flagging a thread on a tiny, unrepresentative sample.
const MIN_RUNS_FOR_RATE: u64 = 5;

/// Default bounded tail size read from `ooda.log` for the error scan.
pub const OODA_TAIL_MAX_BYTES: u64 = 64 * 1024;

/// Detect cognitive-thread anomalies for this Observe pass.
///
/// PURE and panic-free: given the metrics `snapshot`, the name+purpose+cadence
/// `registry` (`Mind::health()`), a bounded `ooda_tail`, and `now` (Unix
/// seconds), it returns a bounded list of human-readable anomaly strings. A
/// missing series / empty registry / empty tail simply yields fewer (or no)
/// anomalies — never an error and never a panic.
pub fn detect_thread_anomalies(
    snapshot: &MetricsSnapshot,
    registry: &[ThreadHealth],
    ooda_tail: &[String],
    now: u64,
) -> Vec<String> {
    let mut anomalies: Vec<String> = Vec::new();

    for entry in registry {
        if !entry.enabled {
            // A disabled thread never ticks; it has no telemetry and is not
            // expected to run, so it is never "stalled".
            continue;
        }
        let id = entry.id.as_str();

        // --- Cadence / staleness: next_run_epoch long past its due time. ---
        if let (Some(next_run), Some(cadence)) = (
            snapshot.gauge(&series_name(id, names::THREAD_SUFFIX_NEXT_RUN_EPOCH), &[]),
            entry.cadence_secs,
        ) && next_run > 0
        {
            let grace = cadence
                .saturating_mul(STALL_GRACE_CADENCES)
                .max(MIN_STALL_GRACE_SECS);
            let overdue_after = next_run.saturating_add(grace as i64);
            if (now as i64) > overdue_after {
                // The message is deliberately STABLE per (thread, condition): it
                // names only the thread, its fixed grace/cadence, and its
                // purpose — never the live "N seconds overdue", which grows every
                // cycle. A volatile value here would make `problem.summary`
                // differ each Observe pass, defeating the Overseer's
                // `recipe_dedup_key` / recurrence-recall / observation-write-back
                // gates and re-launching an investigation every cycle for one
                // wedged thread. The investigation reads the live overdue amount
                // from telemetry itself.
                anomalies.push(format!(
                    "cognitive thread '{id}' stalled: scheduled run is overdue past its \
                     {grace}s grace window (cadence {cadence}s) — purpose: {}",
                    entry.purpose
                ));
                // One anomaly per thread per cycle: a stalled thread's failure
                // rate is not independently actionable this pass.
                continue;
            }
        }

        // --- Failure rate: the majority of lifetime attempts failed. ---
        // Note: `runs`/`failures` are cumulative (lifetime) counters, so this
        // check reflects the thread's whole history, not a recent window.
        let runs = snapshot
            .counter(&series_name(id, names::THREAD_SUFFIX_RUNS), &[])
            .unwrap_or(0);
        let failures = snapshot
            .counter(&series_name(id, names::THREAD_SUFFIX_FAILURES), &[])
            .unwrap_or(0);
        if runs >= MIN_RUNS_FOR_RATE && failures.saturating_mul(2) > runs {
            // STABLE per (thread, condition): no live `{failures}/{runs}` counts,
            // which grow every cycle and would defeat cross-cycle dedup (see the
            // stall branch). The investigation reads the live counts from
            // telemetry.
            anomalies.push(format!(
                "cognitive thread '{id}' failing: the majority of its recorded runs have \
                 failed — purpose: {}",
                entry.purpose
            ));
        }
    }

    // --- ooda.log ERROR scan (bounded, single summarized anomaly). ---
    if let Some(error_anomaly) = scan_ooda_errors(ooda_tail) {
        anomalies.push(error_anomaly);
    }

    anomalies.truncate(MAX_THREAD_ANOMALIES_PER_CYCLE);
    anomalies
}

/// Scan a bounded ooda.log tail for ERROR lines, returning a single STABLE
/// anomaly string when any is present, else `None`. The string is deliberately
/// content-free (no count, no excerpt): a live count/excerpt changes every cycle
/// and would defeat the Overseer's cross-cycle dedup (see
/// [`detect_thread_anomalies`]), re-launching an investigation each Observe pass
/// while any error sits in the tail. Omitting the raw log line also keeps
/// untrusted log content out of the notification path entirely (SR-R1). The
/// investigation reads the concrete error lines from `ooda.log` itself.
fn scan_ooda_errors(ooda_tail: &[String]) -> Option<String> {
    let has_error = ooda_tail.iter().any(|line| is_error_line(line));
    has_error.then(|| {
        "ooda.log tail contains recent ERROR line(s) — see the daemon log for detail".to_string()
    })
}

/// Whether a log line is an ERROR line (case-insensitive `error` **token**).
/// Kept deliberately narrow — only the explicit error level, not the broad
/// warning/failure vocabulary — so oversight surfaces genuine errors, not noise.
///
/// Matches `error` only as a whole word, so precision-sapping substrings are
/// rejected: `"0 errors"` (trailing `s` — a word char) and `"error-free"`
/// (trailing `-` — a compound) do **not** count, while `ERROR`, `[error]`, and
/// `error:` do. A word char is ASCII alphanumeric or `_`.
///
/// Allocation-free: scans the line's bytes for the token rather than
/// materializing a lowercased copy of every line (this runs over the whole
/// ooda.log tail, up to [`OODA_TAIL_MAX_BYTES`]).
fn is_error_line(line: &str) -> bool {
    const NEEDLE: &[u8] = b"error";
    let hay = line.as_bytes();
    if hay.len() < NEEDLE.len() {
        return false;
    }
    for start in 0..=hay.len() - NEEDLE.len() {
        if !hay[start..start + NEEDLE.len()].eq_ignore_ascii_case(NEEDLE) {
            continue;
        }
        // Left boundary: the byte before `error` must not be a word char.
        let left_ok = start == 0 || !is_word_byte(hay[start - 1]);
        // Right boundary: the byte after must be neither a word char (excludes
        // `errors`) nor `-` (excludes `error-free`).
        let after = start + NEEDLE.len();
        let right_ok = after >= hay.len() || (!is_word_byte(hay[after]) && hay[after] != b'-');
        if left_ok && right_ok {
            return true;
        }
    }
    false
}

/// Whether `b` is an ASCII "word" byte (alphanumeric or `_`) for token
/// boundary detection in [`is_error_line`].
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Read a bounded tail of `<state_root>/ooda.log` as lines, best-effort.
///
/// Reads at most `max_bytes` from the END of the file (a single `seek`, never a
/// full-file slurp) so an arbitrarily large daemon log can never inflate memory.
/// The (possibly partial) first line is dropped. Any error — missing file,
/// permission, non-UTF-8 — degrades to an empty tail, never a panic.
pub fn read_ooda_tail(state_root: &Path, max_bytes: u64) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};

    let path = state_root.join("ooda.log");
    let read = || -> std::io::Result<(String, u64)> {
        let mut file = std::fs::File::open(&path)?;
        let len = file.metadata()?.len();
        let start = len.saturating_sub(max_bytes);
        file.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::with_capacity(max_bytes.min(len) as usize);
        file.take(max_bytes).read_to_end(&mut buf)?;
        Ok((String::from_utf8_lossy(&buf).into_owned(), start))
    };
    let Ok((content, start)) = read() else {
        return Vec::new();
    };
    let mut it = content.lines();
    // Drop the leading (possibly truncated) partial line only when we actually
    // seeked past the start of the file — skipping it in the iterator avoids
    // both allocating that line and an O(n) Vec shift from `remove(0)`.
    if start > 0 {
        it.next();
    }
    it.map(str::to_string).collect()
}

// ---------------------------------------------------------------------------
// Issue #4845 — TDD contract (Step 7) for the detect → dispatch → dedup path
// (design component C6 / requirement 3). Authored alongside the full-activation
// work: it pins that an injected thread failure, surfaced by
// `detect_thread_anomalies`, folds through the SAME Observe→Orient pipeline
// every other observation uses (`signals_from` → `orient`) into EXACTLY ONE
// deduplicated `ProcessHealth` problem — never a parallel remediation rail
// (#4719: no new scrape path) and never a per-cycle re-dispatch. These tests are
// pure and hermetic (hand-built snapshot + registry), no I/O.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod full_activation_dispatch_tests {
    use super::detect_thread_anomalies;
    use crate::cognitive_threads::ThreadHealth;
    use crate::overseer::capabilities::ObservedState;
    use crate::overseer::orient;
    use crate::overseer::signal::{self, ProblemKind, Signal};
    use crate::telemetry::snapshot::{CounterSeries, GaugeSeries, MetricsSnapshot};

    const NOW: u64 = 2_000_000_000;

    fn tname(id: &str, suffix: &str) -> String {
        format!("simard.thread.{id}.{suffix}")
    }

    fn reg(id: &str, cadence: u64) -> ThreadHealth {
        ThreadHealth {
            id: id.to_string(),
            enabled: true,
            last_run_epoch: None,
            next_run_epoch: None,
            last_success: None,
            consecutive_errors: 0,
            backoff_until_epoch: None,
            purpose: format!("{id} purpose"),
            cadence_secs: Some(cadence),
        }
    }

    /// A snapshot for a thread that has failed the MAJORITY of its runs
    /// (`failed` of `runs`), on-cadence so only the failure-rate branch fires.
    fn failing_snapshot(id: &str, runs: u64, failed: u64) -> MetricsSnapshot {
        let mut snap = MetricsSnapshot::empty();
        snap.gauges.push(GaugeSeries {
            name: tname(id, "next_run_epoch"),
            attrs: Vec::new(),
            value: (NOW + 300) as i64,
        });
        snap.counters.push(CounterSeries {
            name: tname(id, "runs"),
            attrs: Vec::new(),
            value: runs,
        });
        snap.counters.push(CounterSeries {
            name: tname(id, "failures"),
            attrs: Vec::new(),
            value: failed,
        });
        snap
    }

    // C6: an injected failure is DETECTED and dispatches exactly ONE fix.
    #[test]
    fn injected_failure_yields_exactly_one_process_health_dispatch() {
        let id = "engineer_log_analysis";
        let snap = failing_snapshot(id, 10, 8);
        let registry = vec![reg(id, 300)];

        // 1. Detect — the deterministic rail names the failing thread.
        let anomalies = detect_thread_anomalies(&snap, &registry, &[], NOW);
        assert!(
            anomalies.iter().any(|a| a.contains(id)),
            "the injected failing thread must be detected + named: {anomalies:?}"
        );

        // 2. Fold through the SAME Observe→Orient fan-out (no bespoke rail).
        let state = ObservedState {
            anomalies: anomalies.clone(),
            ..ObservedState::default()
        };
        let signals = signal::signals_from(&state);
        assert!(
            signals.iter().any(|s| matches!(s, Signal::Anomaly { .. })),
            "each anomaly string must lift into a Signal::Anomaly"
        );

        // 3. Orient → exactly ONE ProcessHealth problem, keyed `anomaly:<...>`.
        let problems = orient(&signals, &[]);
        let health_problems: Vec<_> = problems
            .iter()
            .filter(|p| p.kind == ProblemKind::ProcessHealth)
            .collect();
        assert_eq!(
            health_problems.len(),
            1,
            "one failing thread ⇒ exactly one remediation problem, got: {problems:?}"
        );
        assert!(
            health_problems[0].dedup_key.starts_with("anomaly:"),
            "the remediation problem is keyed on the stable anomaly signature so the \
             Overseer's recipe_dedup_key suppresses duplicate fix-goals: {}",
            health_problems[0].dedup_key
        );
    }

    // C6: the SAME failure across two Observe passes dedups to ONE dispatch
    // (no per-cycle re-launch for one persistently unhealthy thread).
    #[test]
    fn recurring_failure_dedups_to_a_single_dispatch() {
        let id = "metacognition";
        let registry = vec![reg(id, 300)];

        // Two cycles, DIFFERENT live magnitudes (8/10 then 30/40 failures) —
        // the detector must emit a byte-identical stable string both times
        // (the signature is content-free w.r.t. the raw counts, so dedup holds).
        let a = detect_thread_anomalies(&failing_snapshot(id, 10, 8), &registry, &[], NOW);
        let b = detect_thread_anomalies(&failing_snapshot(id, 40, 30), &registry, &[], NOW);
        assert_eq!(a, b, "same condition ⇒ identical stable anomaly string");

        // Both cycles' signals in Orient at once ⇒ still ONE problem (merged by
        // dedup_key), i.e. exactly one fix is dispatched, not one per cycle.
        let state = ObservedState {
            anomalies: [a, b].concat(),
            ..ObservedState::default()
        };
        let signals = signal::signals_from(&state);
        let problems = orient(&signals, &[]);
        let health = problems
            .iter()
            .filter(|p| p.kind == ProblemKind::ProcessHealth)
            .count();
        assert_eq!(
            health, 1,
            "a recurring failure signature dedups to a single dispatch, got {health} problems"
        );
    }

    // Dual-CHANNEL detection: the failure-rate verdict is driven by the real
    // per-thread telemetry series. Drop the `failures` counter and the same
    // thread is no longer flagged — proving detection reads live telemetry, not
    // a hardcoded verdict (zero-BS).
    #[test]
    fn detection_is_driven_by_the_real_failures_counter() {
        let id = "salience";
        let registry = vec![reg(id, 300)];

        let flagged = detect_thread_anomalies(&failing_snapshot(id, 10, 8), &registry, &[], NOW);
        assert!(
            flagged.iter().any(|x| x.contains(id)),
            "with a majority-failure counter the thread is flagged"
        );

        // Same thread, but only 1 of 10 failed ⇒ healthy ⇒ not flagged.
        let healthy = detect_thread_anomalies(&failing_snapshot(id, 10, 1), &registry, &[], NOW);
        assert!(
            !healthy.iter().any(|x| x.contains(id)),
            "a mostly-succeeding thread is NOT flagged (verdict tracks real counts): {healthy:?}"
        );
    }
}
