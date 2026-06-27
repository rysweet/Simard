use super::*;
use serial_test::serial;
use std::env;

/// Helper: set HOME to a temp dir so tests don't pollute the real home.
fn with_temp_home<F: FnOnce()>(f: F) {
    let dir = env::current_dir()
        .unwrap()
        .join("target")
        .join("test-metrics-home");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    // Temporarily override HOME
    let prev = env::var_os("HOME");
    // SAFETY: tests using this helper are run serially (single-threaded
    // within this module) and restore HOME afterwards.
    unsafe { env::set_var("HOME", &dir) };
    f();
    // Restore HOME
    match prev {
        Some(v) => unsafe { env::set_var("HOME", v) },
        None => unsafe { env::remove_var("HOME") },
    }
    let _ = fs::remove_dir_all(&dir);
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
