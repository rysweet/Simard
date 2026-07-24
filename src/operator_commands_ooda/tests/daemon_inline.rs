use crate::operator_commands_ooda::daemon::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

#[test]
fn test_interruptible_sleep_returns_immediately_on_shutdown() {
    let shutdown = AtomicBool::new(true);
    let start = Instant::now();
    interruptible_sleep(Duration::from_secs(60), &shutdown);
    // Deflake #4560: 1s → 5s tolerates canary CPU oversubscription; still a
    // 12x margin below the 60s sleep this early-return guards against.
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn test_interruptible_sleep_completes_short_duration() {
    let shutdown = AtomicBool::new(false);
    let start = Instant::now();
    interruptible_sleep(Duration::from_millis(100), &shutdown);
    assert!(start.elapsed() >= Duration::from_millis(100));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn test_interruptible_sleep_zero_duration() {
    let shutdown = AtomicBool::new(false);
    let start = Instant::now();
    interruptible_sleep(Duration::ZERO, &shutdown);
    assert!(start.elapsed() < Duration::from_millis(50));
}

#[test]
fn test_binary_changed_false_for_future_time() {
    // If start_time is far in the future, binary should not appear changed.
    let future = SystemTime::now() + Duration::from_secs(86400 * 365 * 10);
    assert!(!binary_changed(future));
}

#[test]
fn test_exe_mtime_returns_some() {
    // The test binary itself should have a valid mtime.
    let mtime = exe_mtime();
    assert!(mtime.is_some());
}

#[test]
fn test_binary_changed_false_for_epoch_identical_content() {
    // Content-identity gate (2026-07-02 operator-review #2): even though an
    // epoch start_time trips the mtime pre-filter, the on-disk binary is the
    // running test binary (identical content), so no relaunch. The prior
    // mtime-only check asserted `true` here — that was the self-restart churn
    // bug this gate fixes.
    let epoch = SystemTime::UNIX_EPOCH;
    assert!(!binary_changed(epoch));
}

#[test]
fn test_interruptible_sleep_mid_shutdown() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&shutdown);
    // Set shutdown after 100ms
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        flag.store(true, Ordering::SeqCst);
    });
    let start = Instant::now();
    interruptible_sleep(Duration::from_secs(60), &shutdown);
    // Should return well before 60s
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn daemon_dashboard_config_default_values() {
    // Clear any env override to test the true default
    unsafe { std::env::remove_var("SIMARD_DASHBOARD_PORT") };
    let config = DaemonDashboardConfig::default();
    assert!(config.enabled);
    assert_eq!(config.port, 8080);
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn daemon_dashboard_config_env_override() {
    unsafe { std::env::set_var("SIMARD_DASHBOARD_PORT", "9090") };
    let config = DaemonDashboardConfig::default();
    assert_eq!(config.port, 9090);
    // Clean up
    unsafe { std::env::remove_var("SIMARD_DASHBOARD_PORT") };
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn daemon_dashboard_config_invalid_env_falls_back() {
    unsafe { std::env::set_var("SIMARD_DASHBOARD_PORT", "not_a_number") };
    let config = DaemonDashboardConfig::default();
    assert_eq!(config.port, 8080);
    unsafe { std::env::remove_var("SIMARD_DASHBOARD_PORT") };
}

#[test]
fn daemon_log_writes_to_stderr_and_file() {
    let dir = tempfile::tempdir().unwrap();
    daemon_log(dir.path(), "test daemon log message");
    let log_path = dir.path().join("ooda.log");
    assert!(log_path.is_file());
    let contents = std::fs::read_to_string(&log_path).unwrap();
    assert!(contents.contains("test daemon log message"));
}

#[test]
fn daemon_log_appends_not_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    daemon_log(dir.path(), "first message");
    daemon_log(dir.path(), "second message");
    let contents = std::fs::read_to_string(dir.path().join("ooda.log")).unwrap();
    assert!(contents.contains("first message"));
    assert!(contents.contains("second message"));
}

#[test]
fn exe_mtime_is_in_reasonable_range() {
    let mtime = exe_mtime().unwrap();
    let elapsed = mtime.elapsed().unwrap_or(Duration::ZERO);
    // The test binary should have been built recently (within last year)
    assert!(elapsed < Duration::from_secs(365 * 24 * 3600));
}

#[test]
fn daemon_log_creates_file_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("ooda.log");
    assert!(!log_path.exists());
    daemon_log(dir.path(), "creation test");
    assert!(log_path.exists());
}

#[test]
fn daemon_log_includes_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    daemon_log(dir.path(), "timestamped message");
    let contents = std::fs::read_to_string(dir.path().join("ooda.log")).unwrap();
    // Timestamp format: YYYY-MM-DDTHH:MM:SSZ
    assert!(
        contents.contains('T') && contents.contains('Z'),
        "log should contain ISO timestamp, got: {contents}"
    );
}

#[test]
fn binary_changed_false_for_current_time() {
    // If start_time is now, binary should not appear changed.
    assert!(!binary_changed(SystemTime::now()));
}

#[test]
fn interruptible_sleep_very_short_duration() {
    let shutdown = AtomicBool::new(false);
    let start = Instant::now();
    interruptible_sleep(Duration::from_millis(1), &shutdown);
    // Deflake #4560: 1s → 5s tolerates canary CPU oversubscription for this
    // ~1ms sleep's completion deadline.
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn dashboard_config_fields_are_independent() {
    let config = DaemonDashboardConfig {
        enabled: false,
        port: 3000,
    };
    assert!(!config.enabled);
    assert_eq!(config.port, 3000);
}

// ── Deflake #4560b: oversubscription-tolerant interruptible_sleep bounds ──────
//
// The self-deploy canary gate flaked because the shutdown fast-return and
// very-short-completion assertions used a 1s ceiling that does not hold when
// full-parallel canaries oversubscribe the CPU (observed load avg 73–158). The
// robust contract is a 5s ceiling: still 12x below the 60s guarded sleep, so a
// genuine no-wake hang is caught, but tolerant of scheduler pressure. These
// tests pin that contract under actively induced oversubscription and repeat to
// expose any residual flake.

/// Spins up CPU-hog threads to simulate the full-parallel canary
/// oversubscription under which the 1s bounds flaked. Stops and joins on drop.
struct CpuHogs {
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl CpuHogs {
    fn spawn() -> Self {
        let count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let stop = Arc::new(AtomicBool::new(false));
        let handles = (0..count)
            .map(|_| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
            })
            .collect();
        Self { stop, handles }
    }
}

impl Drop for CpuHogs {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

#[test]
fn interruptible_sleep_shutdown_return_is_oversubscription_tolerant() {
    let _hogs = CpuHogs::spawn();
    for i in 0..20 {
        let shutdown = AtomicBool::new(true);
        let start = Instant::now();
        interruptible_sleep(Duration::from_secs(60), &shutdown);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "iteration {i}: already-shutdown sleep must fast-return under oversubscription, took {elapsed:?}"
        );
    }
}

#[test]
fn interruptible_sleep_short_completion_is_oversubscription_tolerant() {
    let _hogs = CpuHogs::spawn();
    for i in 0..20 {
        let shutdown = AtomicBool::new(false);
        let start = Instant::now();
        interruptible_sleep(Duration::from_millis(1), &shutdown);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "iteration {i}: very-short sleep must complete promptly under oversubscription, took {elapsed:?}"
        );
    }
}
