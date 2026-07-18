//! Cost tracking ledger for LLM calls.
//!
//! Records estimated token usage and cost for each session turn into a
//! JSON-lines file at `~/.simard/costs/ledger.jsonl`.  Provides helpers
//! to query daily and weekly summaries.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

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

#[cfg(test)]
thread_local! {
    /// Test-only, thread-local cost-ledger path override consulted first by
    /// [`ledger_path`]. Because it is thread-local, concurrently-running tests
    /// on other threads never observe it — this is the race-proof replacement
    /// for mutating the process-global `HOME` to isolate the ledger.
    static LEDGER_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Test-only RAII guard that redirects the cost ledger to `target` for the
/// current thread only, restoring the previous resolution when dropped.
///
/// Unlike mutating `HOME`, this cannot race with concurrent tests: the
/// override lives in a thread-local, so a meeting-cost test can isolate its
/// ledger without a serial key and without tearing another test's environment.
/// `Drop` always runs (including on panic), so the override is never leaked.
#[cfg(test)]
pub(crate) struct LedgerPathGuard {
    _private: (),
}

#[cfg(test)]
impl LedgerPathGuard {
    /// Redirect [`ledger_path`] to `target` on the current thread until the
    /// returned guard is dropped.
    pub(crate) fn set(target: &std::path::Path) -> Self {
        LEDGER_PATH_OVERRIDE.with(|cell| {
            *cell.borrow_mut() = Some(target.to_path_buf());
        });
        LedgerPathGuard { _private: () }
    }
}

#[cfg(test)]
impl Drop for LedgerPathGuard {
    fn drop(&mut self) {
        LEDGER_PATH_OVERRIDE.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}

/// Resolve the cost-ledger path. In test builds the resolution order is:
/// 1. a thread-local [`LedgerPathGuard`] override;
/// 2. the `SIMARD_COST_LEDGER` environment variable, if set;
/// 3. the `$HOME`-derived default `~/.simard/costs/ledger.jsonl`.
///
/// Both override branches are strictly `#[cfg(test)]`-gated: a non-test build
/// resolves the ledger from `$HOME` exactly as before, so the production
/// cost-tracking path is byte-for-byte unchanged.
fn ledger_path() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(path) = LEDGER_PATH_OVERRIDE.with(|cell| cell.borrow().clone()) {
            return path;
        }
        if let Some(path) = std::env::var_os("SIMARD_COST_LEDGER") {
            return PathBuf::from(path);
        }
    }
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
    let path = ledger_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    let line = serde_json::to_string(entry).map_err(|e| std::io::Error::other(e.to_string()))?;
    writeln!(file, "{}", line)?;
    Ok(())
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
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-cost-tracking");
        fs::create_dir_all(&dir).unwrap();
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
        fs::remove_dir_all(&dir).ok();
    }

    // ── Brick B: test-only cost-ledger path override (issue #4322 de-flake) ──
    //
    // These tests specify the contract for the `LedgerPathGuard` thread-local
    // seam that makes the meeting-turn cost test race-proof: `ledger_path()`
    // must consult a thread-local override first, then a `SIMARD_COST_LEDGER`
    // env fallback, then the `$HOME`-derived default. They are written before
    // the implementation (TDD) and therefore FAIL to compile until the seam
    // (`LedgerPathGuard`, `LEDGER_PATH_OVERRIDE`, and the `SIMARD_COST_LEDGER`
    // branch in `ledger_path()`) exists. Once implemented they must pass.

    #[test]
    fn ledger_guard_redirects_ledger_path_and_clears_on_drop() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("nested").join("ledger.jsonl");

        // Before any guard, resolution is the HOME-derived default — never our
        // unique temp target.
        assert_ne!(ledger_path(), target);

        {
            let _guard = LedgerPathGuard::set(&target);
            assert_eq!(
                ledger_path(),
                target,
                "an active LedgerPathGuard must redirect ledger_path() to its target"
            );
        }

        // Dropping the guard (including via panic, since Drop always runs)
        // clears the thread-local override and restores default resolution.
        assert_ne!(
            ledger_path(),
            target,
            "dropping the guard must clear the thread-local override"
        );
    }

    #[test]
    fn ledger_guard_redirects_record_cost_writes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("costs").join("ledger.jsonl");
        let _guard = LedgerPathGuard::set(&target);

        let session_id = "guard-redirect-session";
        record_cost(session_id, "test-model", 400, 40, "brick-b unit test")
            .expect("record_cost must succeed writing to the guarded path");

        let contents = fs::read_to_string(&target)
            .expect("record_cost must write the entry to the guarded ledger path");
        let found = contents
            .lines()
            .filter_map(|l| serde_json::from_str::<CostEntry>(l).ok())
            .any(|e| e.session_id == session_id && e.model == "test-model");
        assert!(
            found,
            "the guarded ledger must contain the entry recorded through record_cost"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn ledger_guard_takes_precedence_over_env_fallback() {
        let tmp = tempfile::TempDir::new().unwrap();
        let guard_target = tmp.path().join("thread-local.jsonl");
        let env_target = tmp.path().join("env-fallback.jsonl");

        // SAFETY: serialised via the cognitive_memory key so no concurrent test
        // observes this transient env mutation; it is cleared before returning.
        unsafe {
            std::env::set_var("SIMARD_COST_LEDGER", &env_target);
        }
        let resolved = {
            let _guard = LedgerPathGuard::set(&guard_target);
            ledger_path()
        };
        unsafe {
            std::env::remove_var("SIMARD_COST_LEDGER");
        }

        assert_eq!(
            resolved, guard_target,
            "thread-local override must take precedence over SIMARD_COST_LEDGER"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn simard_cost_ledger_env_overrides_default_when_no_guard() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env_target = tmp.path().join("env-only.jsonl");

        // SAFETY: serialised via the cognitive_memory key so the process-global
        // env is not torn between set and read; cleared before returning.
        unsafe {
            std::env::set_var("SIMARD_COST_LEDGER", &env_target);
        }
        let resolved = ledger_path();
        unsafe {
            std::env::remove_var("SIMARD_COST_LEDGER");
        }

        assert_eq!(
            resolved, env_target,
            "SIMARD_COST_LEDGER must override the default path when no guard is set"
        );
    }

    #[test]
    fn record_cost_surfaces_unwritable_ledger_as_err() {
        // Point the ledger at a path whose parent is an existing *file*, so
        // create_dir_all / open must fail. record_cost has to surface this as
        // Err — which the meeting path then logs via tracing::warn! — rather
        // than silently presenting a failed write as "entry not recorded".
        let tmp = tempfile::TempDir::new().unwrap();
        let file_as_parent = tmp.path().join("not-a-dir");
        fs::write(&file_as_parent, b"x").unwrap();
        let bad = file_as_parent.join("costs").join("ledger.jsonl");

        let _guard = LedgerPathGuard::set(&bad);
        let result = record_cost("err-session", "m", 10, 5, "unwritable ledger");
        assert!(
            result.is_err(),
            "record_cost must return Err when the ledger path is unwritable"
        );
    }
}
