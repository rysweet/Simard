//! Outside-in consumer test for the streaming cost-summary path.
//!
//! Exercises the public `simard::daily_summary` / `simard::weekly_summary`
//! boundary exactly as the OODA loop and operator dashboard do, driving a real
//! on-disk JSONL ledger through the `SIMARD_COST_LEDGER_PATH` override. Verifies
//! that the streaming aggregation (issue #4355) correctly skips empty and
//! malformed lines and applies the daily/weekly time-window filters without
//! materializing the full entry list.

use chrono::{Duration, Utc};
use simard::{CostEntry, daily_summary, weekly_summary};
use std::io::Write;

fn entry(ts_offset: Duration, prompt: u64, completion: u64, cost: f64) -> CostEntry {
    CostEntry {
        timestamp: Utc::now() + ts_offset,
        session_id: "sess-outside-in".to_string(),
        model: "test-model".to_string(),
        prompt_tokens_est: prompt,
        completion_tokens_est: completion,
        cost_usd_est: cost,
        context: "qa-outside-in".to_string(),
    }
}

#[test]
fn streaming_summaries_filter_and_aggregate_from_ledger() {
    // Unique per-process ledger so parallel test binaries never collide.
    let ledger = std::env::temp_dir().join(format!(
        "simard-cost-ledger-outside-in-{}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&ledger);

    // Two entries "today", one within the week (2 days ago), one outside the
    // week (10 days ago), interleaved with blank and malformed lines that the
    // streaming reader must silently skip.
    let today_a = entry(Duration::zero(), 100, 200, 1.0);
    let today_b = entry(Duration::minutes(-5), 50, 25, 0.5);
    let this_week = entry(Duration::days(-2), 30, 10, 0.25);
    let last_month = entry(Duration::days(-10), 999, 999, 99.0);

    {
        let mut f = std::fs::File::create(&ledger).expect("create ledger");
        writeln!(f).unwrap();
        writeln!(f, "{}", serde_json::to_string(&today_a).unwrap()).unwrap();
        writeln!(f, "   ").unwrap();
        writeln!(f, "this is not json at all").unwrap();
        writeln!(f, "{}", serde_json::to_string(&today_b).unwrap()).unwrap();
        writeln!(f, "{{\"partial\": true").unwrap();
        writeln!(f, "{}", serde_json::to_string(&this_week).unwrap()).unwrap();
        writeln!(f, "{}", serde_json::to_string(&last_month).unwrap()).unwrap();
    }

    // SAFETY: single-threaded test; no concurrent env mutation in this binary.
    unsafe {
        std::env::set_var("SIMARD_COST_LEDGER_PATH", &ledger);
    }

    let daily = daily_summary().expect("daily_summary");
    let weekly = weekly_summary().expect("weekly_summary");

    // SAFETY: single-threaded test cleanup.
    unsafe {
        std::env::remove_var("SIMARD_COST_LEDGER_PATH");
    }
    let _ = std::fs::remove_file(&ledger);

    // Daily: only the two "today" entries survive the malformed/blank lines and
    // the date filter.
    assert_eq!(
        daily.entry_count, 2,
        "daily should count exactly today's entries"
    );
    assert_eq!(daily.total_prompt_tokens, 150);
    assert_eq!(daily.total_completion_tokens, 225);
    assert!((daily.total_cost_usd - 1.5).abs() < 1e-9);

    // Weekly: today's two entries plus the 2-days-ago entry; the 10-days-ago
    // entry is outside the 7-day window and must be excluded.
    assert_eq!(
        weekly.entry_count, 3,
        "weekly should exclude the out-of-window entry"
    );
    assert_eq!(weekly.total_prompt_tokens, 180);
    assert_eq!(weekly.total_completion_tokens, 235);
    assert!((weekly.total_cost_usd - 1.75).abs() < 1e-9);
}
