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

/// Resolve the absolute path to the cost ledger,
/// `<home>/.simard/costs/ledger.jsonl`.
///
/// The home directory is resolved portably (issue #4363):
///   1. `HOME` (non-empty) — the unchanged, primary source on Unix,
///   2. `dirs::home_dir()` — the platform home-directory API (portable across
///      Unix and Windows),
///   3. a process-relative `.simard/costs/ledger.jsonl` fallback.
///
/// This never panics and never returns the machine-specific `/home/azureuser`
/// literal that the pre-fix implementation hardcoded. It emits a
/// `tracing::warn!` only when it must fall back to the process-relative path.
/// The signature and return type are unchanged from before #4363, so every
/// existing caller keeps working.
fn ledger_path() -> PathBuf {
    ledger_home()
        .join(".simard")
        .join("costs")
        .join("ledger.jsonl")
}

/// Resolve the home directory that anchors the cost ledger. See [`ledger_path`]
/// for the full resolution chain and degrade-safe contract.
fn ledger_home() -> PathBuf {
    // 1. Honor a non-empty HOME exactly as before (empty is treated as unset so
    //    the ledger never resolves to the filesystem root `/`).
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home);
    }

    // 2. Fall back to the platform home-directory API. On Unix this consults the
    //    OS user database when HOME is absent; on Windows it resolves the
    //    profile folder — this is what makes resolution portable rather than
    //    machine-specific.
    if let Some(home) = dirs::home_dir()
        && !home.as_os_str().is_empty()
    {
        return home;
    }

    // 3. Last resort: a process-relative `.simard/costs` directory. Never a
    //    hardcoded absolute path, never `/tmp`, and never the filesystem root.
    tracing::warn!(
        target: "cost_tracking",
        "no home directory resolved (HOME unset/empty and dirs::home_dir() \
         unavailable); writing the cost ledger to a process-relative \
         .simard/costs directory"
    );
    PathBuf::from(".")
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
        create_ledger_dirs(parent)?;
        harden_ledger_dirs(parent);
    }
    let mut file = open_ledger_file(&path)?;
    harden_ledger_file(&file);
    let line = serde_json::to_string(entry).map_err(|e| std::io::Error::other(e.to_string()))?;
    writeln!(file, "{}", line)?;
    Ok(())
}

/// Create the ledger directory chain, applying the owner-only (`0700`) mode
/// *at creation time* on Unix so there is no window where the freshly created
/// `.simard`/`.simard/costs` directories are world-traversable under a lax
/// umask. [`harden_ledger_dirs`] still runs afterwards to guarantee the exact
/// mode on directories that already existed.
#[cfg(unix)]
fn create_ledger_dirs(parent: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
}

#[cfg(not(unix))]
fn create_ledger_dirs(parent: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(parent)
}

/// Open (creating if needed) the append-only ledger file. On Unix the file is
/// created with mode `0600` *atomically* so a newly created ledger is never
/// briefly world-readable before [`harden_ledger_file`] tightens it. The mode
/// only applies to files this call creates; the post-open chmod still enforces
/// `0600` on pre-existing ledgers.
fn open_ledger_file(path: &std::path::Path) -> std::io::Result<fs::File> {
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// Tighten the `.simard/costs` (and its parent `.simard`) directories to
/// owner-only (`0700`) on Unix. The cost ledger holds session telemetry and is
/// treated as private. A failure never aborts a session turn (the directories
/// still exist and are usable), but it is surfaced via `tracing::warn!` so a
/// degraded-permissions state is observable rather than silent.
#[cfg(unix)]
fn harden_ledger_dirs(costs_dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    for dir in [Some(costs_dir), costs_dir.parent()].into_iter().flatten() {
        if let Err(e) = fs::set_permissions(dir, fs::Permissions::from_mode(0o700)) {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "failed to restrict cost-ledger directory to 0700; continuing with existing permissions"
            );
        }
    }
}

#[cfg(not(unix))]
fn harden_ledger_dirs(_costs_dir: &std::path::Path) {}

/// Tighten the ledger file to owner read/write only (`0600`) on Unix. A failure
/// is degrade-safe like [`harden_ledger_dirs`] and surfaced via `tracing::warn!`
/// so a weakened file mode is observable rather than silent.
#[cfg(unix)]
fn harden_ledger_file(file: &fs::File) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = file.set_permissions(fs::Permissions::from_mode(0o600)) {
        tracing::warn!(
            error = %e,
            "failed to restrict cost-ledger file to 0600; continuing with existing permissions"
        );
    }
}

#[cfg(not(unix))]
fn harden_ledger_file(_file: &fs::File) {}

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

    // ---- Issue #4363 (P4): portable ledger_path() HOME resolution ----
    //
    // Pre-fix, `ledger_path()` hardcoded a "/home/azureuser" fallback when HOME
    // was unset (and produced a *relative* ".simard/..." path when HOME was
    // empty). These tests pin the portable contract from the design doc:
    //   * HOME-set is honored unchanged (non-breaking).
    //   * HOME-unset/empty resolves via the portable `dirs::home_dir()` API to
    //     an ABSOLUTE ledger path — never the old hardcoded literal or a
    //     relative path.
    //
    // NOTE on assertions: a bare `!contains("/home/azureuser")` check is
    // environment-fragile — on a host whose real home genuinely *is*
    // /home/azureuser, the correct portable resolution legitimately returns it.
    // So instead we assert equality with the `dirs::home_dir()`-derived path
    // (the portable API), which fails the old hardcoded code on any host where
    // the real home differs (e.g. CI's /home/runner) while remaining correct
    // everywhere.
    //
    // Env vars are process-global, so all env-mutating tests here serialize on
    // a shared mutex to avoid cross-test flakiness. Edition 2024 makes
    // set_var/remove_var `unsafe`, hence the explicit unsafe blocks.

    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// The ledger path derived from the portable platform home-directory API,
    /// used as the expected value for the HOME-unset/empty cases.
    fn dirs_home_ledger() -> PathBuf {
        dirs::home_dir()
            .expect("dirs::home_dir() should resolve a home directory in the test environment")
            .join(".simard")
            .join("costs")
            .join("ledger.jsonl")
    }

    /// RAII guard that restores the prior HOME value on drop.
    struct HomeGuard {
        prev: Option<String>,
    }

    impl HomeGuard {
        fn set(value: Option<&str>) -> Self {
            let prev = std::env::var("HOME").ok();
            match value {
                Some(v) => unsafe { std::env::set_var("HOME", v) },
                None => unsafe { std::env::remove_var("HOME") },
            }
            HomeGuard { prev }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var("HOME", v) },
                None => unsafe { std::env::remove_var("HOME") },
            }
        }
    }

    #[test]
    fn ledger_path_home_unset_uses_portable_home_resolution() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::set(None);

        let path = ledger_path();
        // Resolution goes through the portable `dirs::home_dir()` API rather
        // than the old hardcoded "/home/azureuser" literal. On any host whose
        // real home differs from that literal (e.g. CI), this equality fails
        // the pre-fix implementation and passes the fixed one.
        assert_eq!(
            path,
            dirs_home_ledger(),
            "HOME-unset must resolve via the portable dirs::home_dir() API"
        );
        assert!(
            path.is_absolute(),
            "resolved ledger path must be absolute, got: {}",
            path.display()
        );
        assert!(
            path.to_string_lossy().ends_with("ledger.jsonl"),
            "resolved path must still point at the ledger file, got: {}",
            path.display()
        );
    }

    #[test]
    fn ledger_path_honors_home_when_set() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::set(Some("/tmp/simard-home-test"));

        let path = ledger_path();
        let expected = PathBuf::from("/tmp/simard-home-test")
            .join(".simard")
            .join("costs")
            .join("ledger.jsonl");
        assert_eq!(
            path, expected,
            "HOME-set behavior must be preserved exactly (non-breaking)"
        );
    }

    // Security regression (Step 10c): the ledger dir/file hardening
    // (`create_ledger_dirs`/`open_ledger_file` + the post-hoc `harden_*`)
    // introduced by #4363 must actually yield owner-only modes. Without this
    // test the atomic-mode fix (commit closing the 0644 TOCTOU window) had no
    // coverage, so a regression to a lax umask would pass unnoticed. Mirrors
    // the established convention in `raw_capture`/`telemetry_facade`.
    #[cfg(unix)]
    #[test]
    fn write_entry_creates_dir_0700_and_file_0600() {
        use std::os::unix::fs::PermissionsExt;

        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(Some(tmp.path().to_str().unwrap()));

        record_cost("sess-perm", "gpt-4", 40, 20, "perm-test")
            .expect("record_cost must write the ledger under the temp HOME");

        let costs_dir = tmp.path().join(".simard").join("costs");
        let ledger = costs_dir.join("ledger.jsonl");

        let dir_mode = fs::metadata(&costs_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "ledger costs dir must be owner-only (0o700), got {dir_mode:o}"
        );

        let dot_dir_mode = fs::metadata(tmp.path().join(".simard"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            dot_dir_mode, 0o700,
            "ledger .simard dir must be owner-only (0o700), got {dot_dir_mode:o}"
        );

        let file_mode = fs::metadata(&ledger).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            file_mode, 0o600,
            "ledger file must be owner rw-only (0o600), got {file_mode:o}"
        );
    }

    #[test]
    fn ledger_path_empty_home_resolves_to_absolute_portable_path() {
        // An empty HOME is treated as unset: it must NOT yield the pre-fix
        // *relative* ".simard/costs/ledger.jsonl" (empty-string join) nor the
        // filesystem root "/.simard/...", but a portable, absolute path.
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::set(Some(""));

        let path = ledger_path();
        assert!(
            path.is_absolute(),
            "empty HOME must resolve to an absolute path, not a relative one, got: {}",
            path.display()
        );
        assert!(
            !path.to_string_lossy().starts_with("/.simard"),
            "empty HOME must not resolve the ledger under the filesystem root, got: {}",
            path.display()
        );
        assert_eq!(
            path,
            dirs_home_ledger(),
            "empty HOME must resolve via the portable dirs::home_dir() API"
        );
    }
}
