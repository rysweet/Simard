use super::*;
use serial_test::serial;
use std::env;

/// Helper: set HOME to a temp dir so tests don't pollute the real home.
///
/// Also **unsets `SIMARD_STATE_ROOT`** for the closure's duration so metric
/// resolution deterministically follows the temp HOME. `metrics_dir()` resolves
/// through `crate::state_root::simard_state_root()`, whose precedence is
/// `SIMARD_STATE_ROOT` → `$HOME/.simard`; sibling tests (and CI) may leave
/// `SIMARD_STATE_ROOT` set, which would otherwise win over the temp HOME and
/// let these tests read/write a shared, uncleaned metrics dir (cross-test
/// contamination). Both env vars are restored on exit.
fn with_temp_home<F: FnOnce()>(f: F) {
    // Unique per-invocation temp HOME. A fixed shared path (e.g.
    // `target/test-metrics-home`) is identical across every parallel *process*
    // running this binary, so concurrent copies would wipe/read each other's
    // metrics dir mid-run (NotFound / dropped entries). A `TempDir` is unique
    // per call and per process, giving true cross-process isolation.
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().to_path_buf();
    // Temporarily override HOME and clear any inherited/leaked state-root env.
    let prev = env::var_os("HOME");
    let prev_state_root = env::var_os(crate::state_root::STATE_ROOT_ENV);
    // SAFETY: tests using this helper are serialised via `#[serial(cognitive_memory)]`
    // (the crate's env-mutation key) and restore both vars afterwards.
    unsafe {
        env::set_var("HOME", &dir);
        env::remove_var(crate::state_root::STATE_ROOT_ENV);
    }
    f();
    // Restore HOME and SIMARD_STATE_ROOT
    match prev {
        Some(v) => unsafe { env::set_var("HOME", v) },
        None => unsafe { env::remove_var("HOME") },
    }
    match prev_state_root {
        Some(v) => unsafe { env::set_var(crate::state_root::STATE_ROOT_ENV, v) },
        None => unsafe { env::remove_var(crate::state_root::STATE_ROOT_ENV) },
    }
    // `tmp` is removed automatically on drop.
}

#[test]
fn metric_entry_roundtrip() {
    let entry = MetricEntry {
        timestamp: Utc::now(),
        metric_name: "test_count".to_string(),
        value: 42.0,
        context: "unit test".to_string(),
    };
    let json = serde_json::to_string(&entry).unwrap();
    let parsed: MetricEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.metric_name, "test_count");
    assert!((parsed.value - 42.0).abs() < f64::EPSILON);
}

#[test]
#[serial(cognitive_memory)]
fn record_and_query_metric() {
    with_temp_home(|| {
        record_metric("bugs_fixed", 3.0, "test context").unwrap();
        record_metric("prs_merged", 1.0, "test context").unwrap();
        record_metric("bugs_fixed", 5.0, "later context").unwrap();

        let bugs = query_metrics("bugs_fixed", None).unwrap();
        assert_eq!(bugs.len(), 2);
        assert!((bugs[1].value - 5.0).abs() < f64::EPSILON);

        let prs = query_metrics("prs_merged", None).unwrap();
        assert_eq!(prs.len(), 1);
    });
}

/// Regression test (issue #2419): concurrent `record_metric` appends from many
/// threads must not corrupt or drop records. Before the single-`write_all`
/// fix, `writeln!` on the unbuffered `O_APPEND` file emitted two `write()`
/// syscalls per record (body, then newline), so concurrent writers interleaved
/// into glued/blank lines that the line-by-line readers silently dropped —
/// undercutting the `brain_lifecycle_decision` parse-failure measurement this
/// metric exists to provide. With the fix, every record is one atomic append.
///
/// Uses the `cognitive_memory` serial key (not a bare `#[serial]`): this test
/// mutates `HOME` via `with_temp_home`, so it must share the same lock as every
/// other `HOME`/state-root test to keep env writes off-limits during concurrent
/// env reads (see `test_support::serial_guard`). A bare `#[serial]` would run on
/// a *different* lock, letting this test's 2000 appends race other tests' temp
/// `HOME` and pollute their metrics files.
#[test]
#[serial(cognitive_memory)]
fn concurrent_record_metric_no_corruption_or_loss() {
    with_temp_home(|| {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 250;

        let mut handles = Vec::with_capacity(THREADS);
        for t in 0..THREADS {
            handles.push(std::thread::spawn(move || {
                for i in 0..PER_THREAD {
                    // Non-trivial context (stringified JSON) mirrors the real
                    // brain_lifecycle_decision payload shape and width.
                    let ctx = format!(
                        r#"{{"thread":{t},"seq":{i},"outcome":"parsed","goal_id":"g-{t}-{i}"}}"#
                    );
                    record_metric("brain_lifecycle_decision", 1.0, &ctx).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // Every record must be present and parseable — no glued/dropped lines.
        let entries = query_metrics("brain_lifecycle_decision", None).unwrap();
        assert_eq!(
            entries.len(),
            THREADS * PER_THREAD,
            "all concurrent records must survive intact (no corruption/loss)"
        );

        // The raw file must contain no blank/glued lines either: every
        // non-empty line parses as exactly one MetricEntry.
        let raw = fs::read_to_string(metrics_file_path()).unwrap();
        let mut clean = 0usize;
        for line in raw.lines() {
            if line.trim().is_empty() {
                panic!("found a blank line — indicates an interleaved append");
            }
            serde_json::from_str::<MetricEntry>(line)
                .expect("every line must be a single well-formed MetricEntry");
            clean += 1;
        }
        assert_eq!(clean, THREADS * PER_THREAD);
    });
}

#[test]
#[serial(cognitive_memory)]
fn query_metrics_with_since_filter() {
    with_temp_home(|| {
        record_metric("test_count", 10.0, "old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let cutoff = Utc::now();
        record_metric("test_count", 20.0, "new").unwrap();

        let all = query_metrics("test_count", None).unwrap();
        assert_eq!(all.len(), 2);

        let recent = query_metrics("test_count", Some(cutoff)).unwrap();
        assert_eq!(recent.len(), 1);
        assert!((recent[0].value - 20.0).abs() < f64::EPSILON);
    });
}

#[test]
#[serial(cognitive_memory)]
fn query_metrics_empty_file() {
    with_temp_home(|| {
        let result = query_metrics("nonexistent", None).unwrap();
        assert!(result.is_empty());
    });
}

#[test]
#[serial(cognitive_memory)]
fn daily_report_empty() {
    with_temp_home(|| {
        let report = daily_report().unwrap();
        assert_eq!(report.total_entries, 0);
        assert!(report.bugs_fixed.is_none());
    });
}

#[test]
#[serial(cognitive_memory)]
fn daily_report_with_data() {
    with_temp_home(|| {
        record_metric("bugs_fixed", 2.0, "ctx").unwrap();
        record_metric("prs_merged", 1.0, "ctx").unwrap();
        record_metric("test_count", 150.0, "ctx").unwrap();
        record_metric("cycle_duration_seconds", 30.0, "ctx").unwrap();
        record_metric("cycle_duration_seconds", 50.0, "ctx").unwrap();

        let report = daily_report().unwrap();
        assert_eq!(report.total_entries, 5);
        assert!((report.bugs_fixed.unwrap() - 2.0).abs() < f64::EPSILON);
        assert!((report.prs_merged.unwrap() - 1.0).abs() < f64::EPSILON);
        assert!((report.test_count.unwrap() - 150.0).abs() < f64::EPSILON);
        assert!((report.avg_cycle_duration_secs.unwrap() - 40.0).abs() < f64::EPSILON);
    });
}

#[test]
#[serial(cognitive_memory)]
fn recent_metrics_limit() {
    with_temp_home(|| {
        for i in 0..10 {
            record_metric("test_count", i as f64, "ctx").unwrap();
        }
        let recent = recent_metrics(3).unwrap();
        assert_eq!(recent.len(), 3);
        assert!((recent[0].value - 7.0).abs() < f64::EPSILON);
        assert!((recent[2].value - 9.0).abs() < f64::EPSILON);
    });
}

#[test]
#[serial(cognitive_memory)]
fn collect_and_record_all_records_four_metrics() {
    with_temp_home(|| {
        // collect_and_record_all may fail on gh commands, but it should
        // still create the file and record what it can.
        let _ = collect_and_record_all(Duration::from_secs(42));
        let path = metrics_file_path();
        assert!(path.exists());
        // Should have exactly 4 lines (one per metric).
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 4);
    });
}

#[test]
#[serial(cognitive_memory)]
fn malformed_lines_skipped() {
    with_temp_home(|| {
        let dir = metrics_dir();
        fs::create_dir_all(&dir).unwrap();
        let path = metrics_file_path();
        fs::write(
            &path,
            "not valid json\n{\"timestamp\":\"2025-01-01T00:00:00Z\",\"metric_name\":\"x\",\"value\":1.0,\"context\":\"ok\"}\n",
        )
        .unwrap();
        let entries = query_metrics("x", None).unwrap();
        assert_eq!(entries.len(), 1);
    });
}

/// Regression: the metrics *writer* must honor `SIMARD_STATE_ROOT` so it agrees
/// with the state-root-aware dashboard *reader*.
///
/// Before `metrics_dir()` routed through `crate::state_root::simard_state_root`,
/// it hardcoded `$HOME/.simard/metrics`. That diverged from the dashboard, which
/// reads `metrics/metrics.jsonl` under `simard_state_root()`. The practical
/// symptoms were (1) operators who relocated their state root saw stale/empty
/// cost & brain-failure tabs, and (2) hermetic tests (which set
/// `SIMARD_STATE_ROOT` to a temp dir) leaked fixture metrics into the operator's
/// real `~/.simard/metrics/metrics.jsonl`, permanently polluting the live
/// dashboard's lifetime counters. This test pins the writer to the state root
/// and asserts nothing leaks to `$HOME`.
#[test]
#[serial(cognitive_memory)]
fn record_metric_follows_state_root_not_home() {
    use crate::state_root::STATE_ROOT_ENV;

    let _tmp = tempfile::TempDir::new().unwrap();
    let base = _tmp.path().to_path_buf();
    let home_dir = base.join("home");
    let state_root = base.join("relocated-state");
    fs::create_dir_all(&home_dir).unwrap();
    fs::create_dir_all(&state_root).unwrap();

    let prev_home = env::var_os("HOME");
    let prev_state_root = env::var_os(STATE_ROOT_ENV);
    // SAFETY: keyed into the `cognitive_memory` serial group, so no other test
    // reads/writes these env vars concurrently; both are restored below.
    unsafe {
        env::set_var("HOME", &home_dir);
        env::set_var(STATE_ROOT_ENV, &state_root);
    }

    record_metric("brain_parse_failure", 1.0, "{\"goal_id\":\"regression\"}").unwrap();

    // The metrics file lives under SIMARD_STATE_ROOT, not $HOME/.simard.
    let written = metrics_file_path();
    assert!(
        written.starts_with(&state_root),
        "metrics path {written:?} must be under SIMARD_STATE_ROOT {state_root:?}"
    );
    assert!(
        written.exists(),
        "metrics file must exist under the state root"
    );
    // Nothing must have leaked into $HOME/.simard/metrics.
    let home_metrics = home_dir.join(".simard").join("metrics");
    assert!(
        !home_metrics.exists(),
        "no metrics dir must be created under $HOME when SIMARD_STATE_ROOT is set (found {home_metrics:?})"
    );

    // Restore env before dropping the temp dirs.
    unsafe {
        match prev_home {
            Some(v) => env::set_var("HOME", v),
            None => env::remove_var("HOME"),
        }
        match prev_state_root {
            Some(v) => env::set_var(STATE_ROOT_ENV, v),
            None => env::remove_var(STATE_ROOT_ENV),
        }
    }
}

// ── count_entries_since — pure core of the activity collectors ──────────────
// Regression coverage for the dashboard daily-report bug where prs_merged /
// bugs_fixed were counted from `gh ... --limit 5` with no time filter, so both
// were structurally pinned at a constant 5.0 regardless of real 24h activity.

#[test]
fn count_entries_since_filters_by_window() {
    let now = Utc::now();
    let raw = format!(
        "[{{\"number\":1,\"mergedAt\":\"{}\"}},\
          {{\"number\":2,\"mergedAt\":\"{}\"}},\
          {{\"number\":3,\"mergedAt\":\"{}\"}}]",
        (now - chrono::Duration::hours(1)).to_rfc3339(),
        (now - chrono::Duration::hours(2)).to_rfc3339(),
        (now - chrono::Duration::hours(48)).to_rfc3339(),
    );
    let since = now - chrono::Duration::hours(24);
    // Two of the three merges are inside the 24h window; the 48h-old one is out.
    assert_eq!(count_entries_since(&raw, since, "mergedAt"), 2.0);
}

#[test]
fn count_entries_since_counts_all_in_window_not_capped_at_five() {
    // A busy day well beyond the old --limit 5 cap: every entry is recent.
    let now = Utc::now();
    let recent = (now - chrono::Duration::minutes(30)).to_rfc3339();
    let items: Vec<String> = (0..42)
        .map(|n| format!("{{\"number\":{n},\"mergedAt\":\"{recent}\"}}"))
        .collect();
    let raw = format!("[{}]", items.join(","));
    let since = now - chrono::Duration::hours(24);
    assert_eq!(count_entries_since(&raw, since, "mergedAt"), 42.0);
}

#[test]
fn count_entries_since_skips_missing_and_unparseable_timestamps() {
    let now = Utc::now();
    let recent = (now - chrono::Duration::hours(1)).to_rfc3339();
    let raw = format!(
        "[{{\"number\":1,\"closedAt\":\"{recent}\"}},\
          {{\"number\":2,\"closedAt\":null}},\
          {{\"number\":3,\"closedAt\":\"not-a-date\"}},\
          {{\"number\":4}}]"
    );
    let since = now - chrono::Duration::hours(24);
    assert_eq!(count_entries_since(&raw, since, "closedAt"), 1.0);
}

#[test]
fn count_entries_since_empty_and_malformed_json() {
    let since = Utc::now() - chrono::Duration::hours(24);
    assert_eq!(count_entries_since("[]", since, "mergedAt"), 0.0);
    assert_eq!(count_entries_since("not json", since, "mergedAt"), 0.0);
}
