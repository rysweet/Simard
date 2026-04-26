//! Self-improvement metrics collection and reporting.
//!
//! Tracks bugs fixed, PRs merged, test count, and cycle duration over time.
//! Metrics are stored as newline-delimited JSON in `~/.simard/metrics/metrics.jsonl`.

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

/// Return the directory where metrics are stored: `~/.simard/metrics/`.
fn metrics_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local"));
    home.join(".simard").join("metrics")
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
    let line = serde_json::to_string(&entry)?;
    writeln!(file, "{line}")?;
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

/// Generate a daily summary report of all metrics recorded in the last 24 hours.
pub fn daily_report() -> Result<DailyReport, Box<dyn std::error::Error>> {
    let since = Utc::now() - chrono::Duration::hours(24);
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
        period_hours: 24,
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

/// Count recently closed bug issues via `gh issue list`.
pub fn collect_bugs_fixed() -> f64 {
    let output = std::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--state",
            "closed",
            "--label",
            "bug",
            "--search",
            "sort:updated-desc",
            "--limit",
            "5",
            "--json",
            "number",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout);
            serde_json::from_str::<Vec<serde_json::Value>>(&raw)
                .map(|v| v.len() as f64)
                .unwrap_or(0.0)
        }
        _ => 0.0,
    }
}

/// Count recently merged PRs via `gh pr list`.
pub fn collect_prs_merged() -> f64 {
    let output = std::process::Command::new("gh")
        .args([
            "pr", "list", "--state", "merged", "--limit", "5", "--json", "number",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout);
            serde_json::from_str::<Vec<serde_json::Value>>(&raw)
                .map(|v| v.len() as f64)
                .unwrap_or(0.0)
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
    record_metric("bugs_fixed", bugs, "closed issues with bug label (last 5)")?;

    let prs = collect_prs_merged();
    record_metric("prs_merged", prs, "recently merged PRs (last 5)")?;

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
