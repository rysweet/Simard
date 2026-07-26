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

/// Max characters of an ooda.log error line echoed into an anomaly string.
const OODA_ERROR_EXCERPT_LEN: usize = 200;

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
                let overdue_secs = (now as i64).saturating_sub(next_run).max(0);
                anomalies.push(format!(
                    "cognitive thread '{id}' stalled: scheduled run is {overdue_secs}s overdue \
                     (cadence {cadence}s) — purpose: {}",
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
            anomalies.push(format!(
                "cognitive thread '{id}' failing: {failures}/{runs} runs failed — purpose: {}",
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

/// Scan a bounded ooda.log tail for ERROR lines, returning a single summarized
/// anomaly (count + a bounded excerpt of the most recent error) or `None`.
/// Summarizing keeps the emission bounded regardless of how many errors the tail
/// holds.
fn scan_ooda_errors(ooda_tail: &[String]) -> Option<String> {
    let mut count = 0usize;
    let mut last: Option<&str> = None;
    for line in ooda_tail {
        if is_error_line(line) {
            count += 1;
            last = Some(line.as_str());
        }
    }
    let last = last?;
    let excerpt = bounded_excerpt(last, OODA_ERROR_EXCERPT_LEN);
    Some(format!(
        "ooda.log shows {count} recent error line(s); latest: {excerpt}"
    ))
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

/// A single-line, length-bounded excerpt of `line` (control chars stripped).
fn bounded_excerpt(line: &str, max: usize) -> String {
    let cleaned: String = line
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max).collect();
        format!("{head}…")
    }
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
