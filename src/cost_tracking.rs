//! Cost tracking ledger for LLM calls.
//!
//! Records estimated token usage and cost for each session turn into a
//! JSON-lines file at `~/.simard/costs/ledger.jsonl`.  Provides helpers
//! to query daily and weekly summaries.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// A single cost entry written to the ledger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CostEntry {
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub model: String,
    pub prompt_tokens_est: u64,
    pub completion_tokens_est: u64,
    pub cost_usd_est: f64,
    pub context: String,
}

/// Aggregated cost summary over a time window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub period: String,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_cost_usd: f64,
    pub entry_count: u64,
}

/// Default cost-per-token used for estimation (USD).
/// Based on a rough average across common models (~$3/1M input, ~$15/1M output).
const DEFAULT_INPUT_COST_PER_TOKEN: f64 = 3.0 / 1_000_000.0;
const DEFAULT_OUTPUT_COST_PER_TOKEN: f64 = 15.0 / 1_000_000.0;

/// Rough character-to-token ratio (4 characters ≈ 1 token).
const CHARS_PER_TOKEN: u64 = 4;

fn ledger_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    PathBuf::from(home)
        .join(".simard")
        .join("costs")
        .join("ledger.jsonl")
}

/// Estimate token count from a character count.
pub fn estimate_tokens(char_count: usize) -> u64 {
    (char_count as u64).saturating_add(CHARS_PER_TOKEN - 1) / CHARS_PER_TOKEN
}

/// Estimate cost from prompt and completion token counts.
pub fn estimate_cost(prompt_tokens: u64, completion_tokens: u64) -> f64 {
    (prompt_tokens as f64 * DEFAULT_INPUT_COST_PER_TOKEN)
        + (completion_tokens as f64 * DEFAULT_OUTPUT_COST_PER_TOKEN)
}

/// Record a cost entry from transcript character sizes.
///
/// `prompt_chars` is the size of the objective/input sent to the LLM.
/// `completion_chars` is the size of the response received.
pub fn record_cost(
    session_id: &str,
    model: &str,
    prompt_chars: usize,
    completion_chars: usize,
    context: &str,
) -> std::io::Result<CostEntry> {
    let prompt_tokens_est = estimate_tokens(prompt_chars);
    let completion_tokens_est = estimate_tokens(completion_chars);
    let cost_usd_est = estimate_cost(prompt_tokens_est, completion_tokens_est);

    tracing::debug!(
        session_id,
        model,
        prompt_tokens_est,
        completion_tokens_est,
        cost_usd_est,
        "recording LLM cost"
    );

    let entry = CostEntry {
        timestamp: Utc::now(),
        session_id: session_id.to_string(),
        model: model.to_string(),
        prompt_tokens_est,
        completion_tokens_est,
        cost_usd_est,
        context: context.to_string(),
    };

    write_entry(&entry)?;

    // Issue #2528: memory token throughput into the unified telemetry facade
    // alongside the authoritative JSONL ledger (which `simard status` reads for
    // the honest $/token/credit reconciliation). Tokens are naturally integral;
    // dollar cost stays ledger-sourced to avoid a lossy integer counter.
    crate::telemetry::counter_add(
        crate::telemetry::names::LLM_TOKENS,
        prompt_tokens_est,
        &[
            (crate::telemetry::names::ATTR_DIR, "in"),
            (crate::telemetry::names::ATTR_CACHED, "false"),
        ],
    );
    crate::telemetry::counter_add(
        crate::telemetry::names::LLM_TOKENS,
        completion_tokens_est,
        &[
            (crate::telemetry::names::ATTR_DIR, "out"),
            (crate::telemetry::names::ATTR_CACHED, "false"),
        ],
    );

    Ok(entry)
}

fn write_entry(entry: &CostEntry) -> std::io::Result<()> {
    let line = serde_json::to_string(entry).map_err(|e| std::io::Error::other(e.to_string()))?;
    append_line(&ledger_path(), &line)
}

/// Append one JSONL record to `path` without ever interleaving with, or
/// dropping relative to, a concurrent append.
///
/// This delegates to [`crate::util::durable_append::append_line`], the single
/// audited source of truth for loss-free `O_APPEND` JSONL writes shared with
/// the self-metrics stream. That helper builds the whole record (body + one
/// newline) and emits it in a single `write_all` under a process-global append
/// mutex, so two in-process threads cannot splice their bytes into one line and
/// (records staying under `PIPE_BUF`) a cross-process appender cannot tear it
/// either. Every IO error propagates — no silent drop.
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    crate::util::durable_append::append_line(path, line)
}

fn read_entries() -> std::io::Result<Vec<CostEntry>> {
    let path = ledger_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path)?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<CostEntry>(trimmed) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn summarize(entries: &[CostEntry], period: &str) -> CostSummary {
    let mut total_prompt = 0u64;
    let mut total_completion = 0u64;
    let mut total_cost = 0.0f64;
    for e in entries {
        total_prompt += e.prompt_tokens_est;
        total_completion += e.completion_tokens_est;
        total_cost += e.cost_usd_est;
    }
    CostSummary {
        period: period.to_string(),
        total_prompt_tokens: total_prompt,
        total_completion_tokens: total_completion,
        total_cost_usd: total_cost,
        entry_count: entries.len() as u64,
    }
}

/// Return a cost summary for today (UTC).
pub fn daily_summary() -> std::io::Result<CostSummary> {
    let entries = read_entries()?;
    let today = Utc::now().date_naive();
    let filtered: Vec<_> = entries
        .into_iter()
        .filter(|e| e.timestamp.date_naive() == today)
        .collect();
    Ok(summarize(&filtered, &format!("daily:{today}")))
}

/// Return a cost summary for the past 7 days (UTC).
pub fn weekly_summary() -> std::io::Result<CostSummary> {
    let entries = read_entries()?;
    let cutoff = Utc::now() - Duration::days(7);
    let filtered: Vec<_> = entries
        .into_iter()
        .filter(|e| e.timestamp > cutoff)
        .collect();
    Ok(summarize(&filtered, "weekly:last-7-days"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(4), 1);
        assert_eq!(estimate_tokens(5), 2);
        assert_eq!(estimate_tokens(8), 2);
        assert_eq!(estimate_tokens(100), 25);
    }

    #[test]
    fn estimate_cost_basic() {
        let cost = estimate_cost(1000, 500);
        let expected =
            1000.0 * DEFAULT_INPUT_COST_PER_TOKEN + 500.0 * DEFAULT_OUTPUT_COST_PER_TOKEN;
        assert!((cost - expected).abs() < 1e-12);
    }

    #[test]
    fn cost_entry_round_trips_through_json() {
        let entry = CostEntry {
            timestamp: Utc::now(),
            session_id: "sess-42".to_string(),
            model: "gpt-4".to_string(),
            prompt_tokens_est: 100,
            completion_tokens_est: 50,
            cost_usd_est: 0.001,
            context: "test turn".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: CostEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn summarize_aggregates_correctly() {
        let entries = vec![
            CostEntry {
                timestamp: Utc::now(),
                session_id: "a".to_string(),
                model: "gpt-4".to_string(),
                prompt_tokens_est: 100,
                completion_tokens_est: 50,
                cost_usd_est: 0.5,
                context: "turn 1".to_string(),
            },
            CostEntry {
                timestamp: Utc::now(),
                session_id: "b".to_string(),
                model: "gpt-4".to_string(),
                prompt_tokens_est: 200,
                completion_tokens_est: 100,
                cost_usd_est: 1.0,
                context: "turn 2".to_string(),
            },
        ];
        let summary = summarize(&entries, "test-period");
        assert_eq!(summary.total_prompt_tokens, 300);
        assert_eq!(summary.total_completion_tokens, 150);
        assert!((summary.total_cost_usd - 1.5).abs() < 1e-12);
        assert_eq!(summary.entry_count, 2);
        assert_eq!(summary.period, "test-period");
    }

    #[test]
    fn read_entries_handles_empty_lines() {
        let _tmp = tempfile::TempDir::new().unwrap();
        let dir = _tmp.path().to_path_buf();
        let path = dir.join("test-empty-lines.jsonl");

        let entry = CostEntry {
            timestamp: Utc::now(),
            session_id: "s1".to_string(),
            model: "m".to_string(),
            prompt_tokens_est: 10,
            completion_tokens_est: 5,
            cost_usd_est: 0.01,
            context: "test".to_string(),
        };
        let json_line = serde_json::to_string(&entry).unwrap();
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "{json_line}").unwrap();
        writeln!(f).unwrap(); // empty line
        writeln!(f, "not-valid-json").unwrap(); // malformed line
        writeln!(f, "{json_line}").unwrap();
        drop(f);

        let file = fs::File::open(&path).unwrap();
        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        for line in reader.lines() {
            let line = line.unwrap();
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(e) = serde_json::from_str::<CostEntry>(trimmed) {
                entries.push(e);
            }
        }
        assert_eq!(entries.len(), 2);
    }

    /// Regression for the intermittent `base_type_copilot::tests::
    /// meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective`
    /// failure ("a copilot-meeting cost entry ... must be recorded").
    ///
    /// Root cause: the meeting-cost test redirects the process-global `HOME`, so
    /// a parallel, non-serialized test that calls `record_cost` (e.g. the
    /// lightweight-chat adapter turn) appended to the SAME ledger concurrently.
    /// The old `writeln!`-based writer emitted the payload and newline as
    /// separate syscalls, so concurrent appends spliced into one another; a
    /// spliced line failed JSON parse and the entry was dropped on read.
    ///
    /// This test hammers the production `append_line` from many threads and
    /// asserts every line round-trips and every entry survives. Reverting
    /// `append_line` to the original lock-free `writeln!` append makes it fail
    /// RED (verified: concurrent two-syscall appends splice into one
    /// unparseable line); the lock plus single-buffer `write_all` makes it
    /// GREEN. The in-process lock alone is sufficient to serialize these
    /// threads, so the additional cross-process protection from the
    /// single-buffer `write_all` is guarded structurally by
    /// `scripts/qa-cost-ledger-concurrent-no-drop.sh`, not by this in-process
    /// test. It writes to a unique per-test `tempfile::TempDir` (no shared
    /// `target/` path, no `HOME` / env dependency), so parallel copies of the
    /// lib test binary cannot collide on the ledger file and it needs no serial
    /// key — it is itself deterministic under massive parallelism.
    #[test]
    fn concurrent_appends_never_interleave_or_drop_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ledger.jsonl");

        const THREADS: usize = 16;
        const PER_THREAD: usize = 64;

        std::thread::scope(|scope| {
            for t in 0..THREADS {
                let path = path.clone();
                scope.spawn(move || {
                    for i in 0..PER_THREAD {
                        let entry = CostEntry {
                            timestamp: Utc::now(),
                            session_id: format!("thread-{t}-entry-{i}"),
                            model: "concurrent-writer".to_string(),
                            prompt_tokens_est: i as u64,
                            completion_tokens_est: t as u64,
                            cost_usd_est: 0.0,
                            context: "x".repeat(64),
                        };
                        let line = serde_json::to_string(&entry).unwrap();
                        append_line(&path, &line).unwrap();
                    }
                });
            }
        });

        let contents = fs::read_to_string(&path).unwrap();
        let mut seen = std::collections::HashSet::new();
        for raw in contents.lines() {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let entry: CostEntry = serde_json::from_str(trimmed).unwrap_or_else(|e| {
                panic!("interleaved/torn ledger line is not valid JSON ({e}): {trimmed:?}")
            });
            assert!(
                seen.insert(entry.session_id.clone()),
                "duplicate ledger entry {:?} — a concurrent append was corrupted",
                entry.session_id
            );
        }
        assert_eq!(
            seen.len(),
            THREADS * PER_THREAD,
            "every concurrently-appended entry must be present and intact"
        );
    }
}
