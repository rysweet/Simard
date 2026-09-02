//! Self-improvement metrics collection and reporting.
//!
//! Tracks bugs fixed, PRs merged, test count, and cycle duration over time.
//! Metrics are stored as newline-delimited JSON in `<state_root>/metrics/metrics.jsonl`,
//! where `<state_root>` follows [`crate::state_root::simard_state_root`]
//! (`SIMARD_STATE_ROOT` when set, else `$HOME/.simard`).

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single metric data point.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MetricEntry {
    pub timestamp: DateTime<Utc>,
    pub metric_name: String,
    pub value: f64,
    pub context: String,
}

/// Return the directory where metrics are stored: `<state_root>/metrics/`.
///
/// Routes through [`crate::state_root::simard_state_root`] so the metrics
/// *writer* honors the same precedence ladder (`SIMARD_STATE_ROOT` →
/// `$HOME/.simard`) as every other state-root-aware caller, including the
/// dashboard *reader* (`/api/brain-failures`, `/api/costs`, `/api/metrics`),
/// which resolves `metrics/metrics.jsonl` under `simard_state_root()`.
///
/// Before this routed through the shared resolver it hardcoded
/// `$HOME/.simard/metrics`, which diverged from the reader in two ways
/// (issue: metrics writer ignored `SIMARD_STATE_ROOT`):
///   1. Operators who relocated their state root via `SIMARD_STATE_ROOT` had
///      metrics written to `$HOME/.simard/metrics` while the dashboard read
///      from `$SIMARD_STATE_ROOT/metrics` — so costs / brain-failures /
///      metrics tabs showed stale or empty data.
///   2. Hermetic tests (which set `SIMARD_STATE_ROOT` to a temp dir) still
///      appended fixture metrics to the operator's real
///      `~/.simard/metrics/metrics.jsonl`, permanently polluting the live
///      dashboard's lifetime counters with unit-test noise.
///
/// Production behavior is unchanged when `SIMARD_STATE_ROOT` is unset, since
/// `simard_state_root()` then resolves to `$HOME/.simard`.
fn metrics_dir() -> PathBuf {
    crate::state_root::simard_state_root().join("metrics")
}

/// Return the path to the metrics JSONL file.
pub fn metrics_file_path() -> PathBuf {
    metrics_dir().join("metrics.jsonl")
}

/// Record a single metric entry, appending it to `metrics.jsonl`.
pub fn record_metric(
    metric_name: &str,
    value: f64,
    context: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let entry = MetricEntry {
        timestamp: Utc::now(),
        metric_name: metric_name.to_string(),
        value,
        context: context.to_string(),
    };
    let dir = metrics_dir();
    fs::create_dir_all(&dir)?;
    let path = metrics_file_path();
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    let mut line = serde_json::to_string(&entry)?;
    line.push('\n');
    // Write the whole record (body + newline) in a single `write_all` so it is
    // ONE `O_APPEND` `write()` syscall, not the two that `writeln!` on an
    // unbuffered file emits (the JSON body, then "\n"). Engineer subprocesses
    // share `$HOME` and append to this file concurrently; a two-syscall write
    // lets records interleave into glued/blank lines, which the line-by-line
    // readers (`query_metrics`/`recent_metrics`/`daily_report`) then silently
    // `continue` past — dropping records. Records are well under `PIPE_BUF`, so
    // a single append write is atomic. Newly consequential now that
    // `brain_lifecycle_decision` is emitted per decision per in-flight engineer
    // (issue #2419), making this the dominant writer to `metrics.jsonl`.
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Query metrics by name, optionally filtered to entries after `since`.
pub fn query_metrics(
    name: &str,
    since: Option<DateTime<Utc>>,
) -> Result<Vec<MetricEntry>, Box<dyn std::error::Error>> {
    let path = metrics_file_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path)?;
    let reader = BufReader::new(file);
    let mut results = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: MetricEntry = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.metric_name != name {
            continue;
        }
        if since
            .as_ref()
            .is_some_and(|cutoff| entry.timestamp < *cutoff)
        {
            continue;
        }
        results.push(entry);
    }
    Ok(results)
}

/// The reporting window, in hours, shared by [`daily_report`] and the
/// activity collectors ([`collect_prs_merged`], [`collect_bugs_fixed`]) so
/// that the counters they record match the period the report claims to cover.
pub const REPORT_WINDOW_HOURS: i64 = 24;

/// Start of the current reporting window (`now - REPORT_WINDOW_HOURS`).
fn report_window_start() -> DateTime<Utc> {
    Utc::now() - chrono::Duration::hours(REPORT_WINDOW_HOURS)
}

/// Generate a daily summary report of all metrics recorded in the last 24 hours.
pub fn daily_report() -> Result<DailyReport, Box<dyn std::error::Error>> {
    let since = Utc::now() - chrono::Duration::hours(REPORT_WINDOW_HOURS);
    let path = metrics_file_path();
    if !path.exists() {
        return Ok(DailyReport::default());
    }
    let file = fs::File::open(&path)?;
    let reader = BufReader::new(file);
    let mut entries: Vec<MetricEntry> = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<MetricEntry>(&line)
            && entry.timestamp >= since
        {
            entries.push(entry);
        }
    }

    let latest = |name: &str| -> Option<f64> {
        entries
            .iter()
            .rfind(|e| e.metric_name == name)
            .map(|e| e.value)
    };

    let avg = |name: &str| -> Option<f64> {
        let vals: Vec<f64> = entries
            .iter()
            .filter(|e| e.metric_name == name)
            .map(|e| e.value)
            .collect();
        if vals.is_empty() {
            None
        } else {
            Some(vals.iter().sum::<f64>() / vals.len() as f64)
        }
    };

    Ok(DailyReport {
        period_hours: REPORT_WINDOW_HOURS as u32,
        bugs_fixed: latest("bugs_fixed"),
        prs_merged: latest("prs_merged"),
        test_count: latest("test_count"),
        avg_cycle_duration_secs: avg("cycle_duration_seconds"),
        total_entries: entries.len(),
    })
}

/// Summary of metrics over a period.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DailyReport {
    pub period_hours: u32,
    pub bugs_fixed: Option<f64>,
    pub prs_merged: Option<f64>,
    pub test_count: Option<f64>,
    pub avg_cycle_duration_secs: Option<f64>,
    pub total_entries: usize,
}

// ---------------------------------------------------------------------------
// Metric collection helpers — gather values from external tools
// ---------------------------------------------------------------------------

/// Upper bound on the number of `gh` records we ask for when counting activity
/// in the reporting window. The `gh --search "…>=<date>"` qualifiers below bound
/// the result set to the window server-side, so this is only a safety ceiling;
/// [`report_window_start`] and [`count_entries_since`] — not the request size —
/// determine the reported count.
const GH_LIST_LIMIT: &str = "500";

/// Count entries in a `gh ... --json` array whose RFC3339 timestamp in `field`
/// falls at or after `since`.
///
/// This is the pure, unit-testable core of [`collect_prs_merged`] and
/// [`collect_bugs_fixed`]. It exists because the historical implementation
/// counted `gh ... --limit 5` rows with **no** time filter, so both metrics
/// were structurally pinned at a constant `5.0` and never reflected the 24-hour
/// window the daily report claims to cover (issue #4256: dashboard daily report
/// under-reports PRs merged / bugs fixed). Entries missing or with an
/// unparseable timestamp are skipped rather than counted.
pub(crate) fn count_entries_since(raw: &str, since: DateTime<Utc>, field: &str) -> f64 {
    serde_json::from_str::<Vec<serde_json::Value>>(raw)
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    item.get(field)
                        .and_then(|v| v.as_str())
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|t| t.with_timezone(&Utc) >= since)
                        .unwrap_or(false)
                })
                .count() as f64
        })
        .unwrap_or(0.0)
}

/// Count bug issues closed within the reporting window via `gh issue list`.
///
/// The `closed:>=<date>` search qualifier is a coarse (day-granularity)
/// server-side pre-filter that bounds the result set to the window; the precise
/// sub-day cutoff is applied by [`count_entries_since`] on `closedAt`.
pub fn collect_bugs_fixed() -> f64 {
    let since = report_window_start();
    let search = format!("closed:>={} sort:updated-desc", since.format("%Y-%m-%d"));
    let output = std::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--state",
            "closed",
            "--label",
            "bug",
            "--search",
            &search,
            "--limit",
            GH_LIST_LIMIT,
            "--json",
            "number,closedAt",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            count_entries_since(&String::from_utf8_lossy(&o.stdout), since, "closedAt")
        }
        _ => 0.0,
    }
}

/// Count PRs merged within the reporting window via `gh pr list`.
///
/// The `is:merged merged:>=<date>` search qualifier bounds the result set to the
/// window server-side — so a PR authored long ago but merged today is counted
/// regardless of how many newer PRs exist — and [`count_entries_since`] applies
/// the precise sub-day cutoff on `mergedAt`.
pub fn collect_prs_merged() -> f64 {
    let since = report_window_start();
    let search = format!("is:merged merged:>={}", since.format("%Y-%m-%d"));
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--search",
            &search,
            "--limit",
            GH_LIST_LIMIT,
            "--json",
            "number,mergedAt",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            count_entries_since(&String::from_utf8_lossy(&o.stdout), since, "mergedAt")
        }
        _ => 0.0,
    }
}

/// Count `#[test]` annotations in the `src/` directory.
pub fn collect_test_count() -> f64 {
    let output = std::process::Command::new("grep")
        .args(["-r", "#[test]", "src/"])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            text.lines().count() as f64
        }
        _ => 0.0,
    }
}

/// Collect all self-improvement metrics and record them.
/// `cycle_duration` is the elapsed wall-clock time for the OODA cycle.
pub fn collect_and_record_all(cycle_duration: Duration) -> Result<(), Box<dyn std::error::Error>> {
    let bugs = collect_bugs_fixed();
    record_metric("bugs_fixed", bugs, "bug-labeled issues closed in last 24h")?;

    let prs = collect_prs_merged();
    record_metric("prs_merged", prs, "PRs merged in last 24h")?;

    let tests = collect_test_count();
    record_metric("test_count", tests, "count of #[test] in src/")?;

    let secs = cycle_duration.as_secs_f64();
    record_metric(
        "cycle_duration_seconds",
        secs,
        "wall-clock duration of OODA cycle",
    )?;

    Ok(())
}

/// Read all metric entries (most recent N). Used by the dashboard endpoint.
pub fn recent_metrics(limit: usize) -> Result<Vec<MetricEntry>, Box<dyn std::error::Error>> {
    let path = metrics_file_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path)?;
    let reader = BufReader::new(file);
    let mut entries: Vec<MetricEntry> = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<MetricEntry>(&line) {
            entries.push(entry);
        }
    }
    // Return the most recent `limit` entries.
    let start = entries.len().saturating_sub(limit);
    Ok(entries[start..].to_vec())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
