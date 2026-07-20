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

/// Resolve the ledger file path.
///
/// A test-only override, `SIMARD_COST_LEDGER_PATH`, takes precedence when set,
/// letting tests pin the ledger to a per-test temp file and avoid racing the
/// process-global `HOME`. When the override is unset (the production case) the
/// path is the unchanged `$HOME/.simard/costs/ledger.jsonl` default.
fn ledger_path() -> PathBuf {
    if let Some(override_path) = std::env::var_os("SIMARD_COST_LEDGER_PATH") {
        tracing::debug!(target: "cost_tracking", "using SIMARD_COST_LEDGER_PATH override");
        return PathBuf::from(override_path);
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

impl CostSummary {
    /// A zeroed summary for `period`, ready to accumulate entries into.
    fn empty(period: String) -> Self {
        Self {
            period,
            total_prompt_tokens: 0,
            total_completion_tokens: 0,
            total_cost_usd: 0.0,
            entry_count: 0,
        }
    }

    /// Fold a single entry's totals into this summary.
    fn add(&mut self, entry: &CostEntry) {
        self.total_prompt_tokens += entry.prompt_tokens_est;
        self.total_completion_tokens += entry.completion_tokens_est;
        self.total_cost_usd += entry.cost_usd_est;
        self.entry_count += 1;
    }
}

/// Stream the ledger once and fold entries matching `keep` into a summary.
///
/// Reads the JSONL file line-by-line and accumulates directly into the
/// `CostSummary`, so peak memory is O(1) in the ledger size rather than
/// materializing (then re-filtering) the full entry list. This matters because
/// `daily_summary`/`weekly_summary` run on the OODA/overseer hot path.
fn summarize_filtered<F>(period: String, keep: F) -> std::io::Result<CostSummary>
where
    F: Fn(&CostEntry) -> bool,
{
    let mut summary = CostSummary::empty(period);
    let path = ledger_path();
    if !path.exists() {
        return Ok(summary);
    }
    let file = fs::File::open(&path)?;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<CostEntry>(trimmed)
            && keep(&entry)
        {
            summary.add(&entry);
        }
    }
    Ok(summary)
}

/// Aggregate a slice of entries under `period` (test-only helper).
#[cfg(test)]
fn summarize(entries: &[CostEntry], period: &str) -> CostSummary {
    let mut summary = CostSummary::empty(period.to_string());
    for entry in entries {
        summary.add(entry);
    }
    summary
}

/// Return a cost summary for today (UTC).
pub fn daily_summary() -> std::io::Result<CostSummary> {
    let today = Utc::now().date_naive();
    summarize_filtered(format!("daily:{today}"), move |e| {
        e.timestamp.date_naive() == today
    })
}

/// Return a cost summary for the past 7 days (UTC).
pub fn weekly_summary() -> std::io::Result<CostSummary> {
    let cutoff = Utc::now() - Duration::days(7);
    summarize_filtered("weekly:last-7-days".to_string(), move |e| {
        e.timestamp > cutoff
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Scoped RAII guard that sets an environment variable for the duration of a
    /// test and restores (or removes) its prior value on drop. Env mutation is
    /// process-global, so every test using this guard MUST also carry the
    /// `#[serial_test::serial(cognitive_memory)]` attribute to avoid tearing a
    /// concurrent env write in a parallel test.
    struct EnvVarGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: serialised via `#[serial(cognitive_memory)]` on the caller;
            // no concurrent env mutation can tear this write.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: serialised via `#[serial(cognitive_memory)]` on the caller.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: serialised via `#[serial(cognitive_memory)]` on the caller.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // ledger_path() — SIMARD_COST_LEDGER_PATH override (design: cost_tracking)
    //
    // These tests specify the test-only ledger-path override. They FAIL on the
    // current code (ledger_path ignores SIMARD_COST_LEDGER_PATH and always
    // joins HOME) and PASS once ledger_path resolves the env override ahead of
    // the HOME fallback.
    // -----------------------------------------------------------------------

    /// When `SIMARD_COST_LEDGER_PATH` is set, `ledger_path()` returns that path
    /// verbatim — no `.simard/costs/ledger.jsonl` join, no HOME involvement.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn ledger_path_honors_env_override_verbatim() {
        let tmp = tempfile::TempDir::new().unwrap();
        let override_path = tmp.path().join("pinned-ledger.jsonl");
        let _guard = EnvVarGuard::set("SIMARD_COST_LEDGER_PATH", &override_path);

        assert_eq!(
            ledger_path(),
            override_path,
            "ledger_path() must return SIMARD_COST_LEDGER_PATH verbatim when set"
        );
    }

    /// The override takes precedence over HOME: even with HOME pointing at a
    /// temp dir, the override path (not `$HOME/.simard/costs/ledger.jsonl`) wins.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn ledger_path_override_takes_precedence_over_home() {
        let home = tempfile::TempDir::new().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let override_path = tmp.path().join("elsewhere").join("ledger.jsonl");

        let _home_guard = EnvVarGuard::set("HOME", home.path());
        let _override_guard = EnvVarGuard::set("SIMARD_COST_LEDGER_PATH", &override_path);

        let home_default = home
            .path()
            .join(".simard")
            .join("costs")
            .join("ledger.jsonl");

        let resolved = ledger_path();
        assert_eq!(
            resolved, override_path,
            "override must win over the HOME-based default"
        );
        assert_ne!(
            resolved, home_default,
            "override must NOT resolve to the HOME-based default path"
        );
    }

    /// Production path is unchanged: with the override unset, `ledger_path()`
    /// falls back to `$HOME/.simard/costs/ledger.jsonl`. This is a regression
    /// guard proving the feature is production-inert.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn ledger_path_falls_back_to_home_when_override_unset() {
        let home = tempfile::TempDir::new().unwrap();
        let _override_guard = EnvVarGuard::unset("SIMARD_COST_LEDGER_PATH");
        let _home_guard = EnvVarGuard::set("HOME", home.path());

        let expected = home
            .path()
            .join(".simard")
            .join("costs")
            .join("ledger.jsonl");

        assert_eq!(
            ledger_path(),
            expected,
            "with no override, ledger_path() must use the HOME-based default"
        );
    }

    /// End-to-end: with the override set, `record_cost` writes to the override
    /// file and `daily_summary` reads it back — the whole write/read chain
    /// honors the single `ledger_path()` chokepoint.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn record_cost_writes_and_reads_through_override_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Nested dir to prove write_entry creates parents at the override path.
        let override_path = tmp.path().join("nested").join("ledger.jsonl");
        let _guard = EnvVarGuard::set("SIMARD_COST_LEDGER_PATH", &override_path);

        let entry = record_cost(
            "sess-override-e2e",
            "test-model",
            4000,
            2000,
            "override e2e",
        )
        .expect("record_cost should write to the override path");

        assert!(
            override_path.exists(),
            "record_cost must write the ledger at the override path, not HOME"
        );

        let contents = fs::read_to_string(&override_path).unwrap();
        assert!(
            contents.contains("sess-override-e2e"),
            "override ledger must contain the recorded entry"
        );

        let summary = daily_summary().expect("daily_summary should read the override path");
        assert!(
            summary.entry_count >= 1,
            "daily_summary must count the entry written via the override path"
        );
        assert!(
            summary.total_prompt_tokens >= entry.prompt_tokens_est,
            "daily_summary must aggregate tokens from the override ledger"
        );
    }

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

    /// The streaming summary path (`summarize_filtered`, exercised via
    /// `daily_summary`) must skip blank and malformed JSONL lines and count only
    /// the valid entries — testing the real production code, not a re-implemented
    /// copy of the skip logic.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn summarize_filtered_skips_empty_and_malformed_lines() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("empty-lines.jsonl");
        let _guard = EnvVarGuard::set("SIMARD_COST_LEDGER_PATH", &path);

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

        // daily_summary streams the ledger via summarize_filtered; both entries
        // carry today's timestamp, so only the two valid rows are counted.
        let summary = daily_summary().expect("daily_summary should stream the override ledger");
        assert_eq!(
            summary.entry_count, 2,
            "blank and malformed lines must be skipped, valid entries counted"
        );
        assert_eq!(summary.total_prompt_tokens, 20);
        assert_eq!(summary.total_completion_tokens, 10);
    }
}
