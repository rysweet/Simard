//! Unit tests for disk-pressure checks.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serial_test::serial;

use super::*;

// ------------------------------------------------------------------
// PressureLevel::classify — pure decision logic
// ------------------------------------------------------------------

#[test]
fn classify_ok_when_free_above_threshold() {
    let t = 20 * 1024 * 1024 * 1024; // 20 GiB
    assert_eq!(PressureLevel::classify(t, t), PressureLevel::Ok);
    assert_eq!(PressureLevel::classify(t + 1, t), PressureLevel::Ok);
    assert_eq!(PressureLevel::classify(u64::MAX, t), PressureLevel::Ok);
}

#[test]
fn classify_warn_in_50_to_100_percent_band() {
    let t = 20 * 1024 * 1024 * 1024;
    let half = t / 2;
    assert_eq!(PressureLevel::classify(half, t), PressureLevel::Warn);
    assert_eq!(PressureLevel::classify(half + 1024, t), PressureLevel::Warn);
    assert_eq!(PressureLevel::classify(t - 1, t), PressureLevel::Warn);
}

#[test]
fn classify_refuse_below_50_percent() {
    let t = 20 * 1024 * 1024 * 1024;
    let half = t / 2;
    assert_eq!(PressureLevel::classify(half - 1, t), PressureLevel::Refuse);
    assert_eq!(PressureLevel::classify(0, t), PressureLevel::Refuse);
}

#[test]
fn classify_zero_threshold_collapses_to_ok() {
    // Defensive: if an operator misconfigures the threshold to 0, no
    // free-space check is meaningful — every value classifies as OK.
    // (configured_min_free_gb specifically rejects 0 and falls back to
    // the default; this test just pins the policy's own behavior.)
    assert_eq!(PressureLevel::classify(0, 0), PressureLevel::Ok);
    assert_eq!(PressureLevel::classify(1024, 0), PressureLevel::Ok);
}

// ------------------------------------------------------------------
// check_disk_pressure_with via a fake provider
// ------------------------------------------------------------------

struct FakeProvider {
    stats: Mutex<Vec<DiskStat>>,
}

impl FakeProvider {
    fn new(stats: Vec<DiskStat>) -> Self {
        Self {
            stats: Mutex::new(stats),
        }
    }
}

impl DiskStatProvider for FakeProvider {
    fn stat(&self, _path: &Path) -> Result<DiskStat, std::io::Error> {
        let mut stats = self.stats.lock().unwrap();
        if stats.is_empty() {
            return Err(std::io::Error::other("no more synthetic stats"));
        }
        Ok(stats.remove(0))
    }
}

#[test]
fn check_disk_pressure_with_returns_ok_band() {
    let p = FakeProvider::new(vec![DiskStat {
        free_bytes: 50 * 1024 * 1024 * 1024,
        total_bytes: 100 * 1024 * 1024 * 1024,
    }]);
    let r = check_disk_pressure_with(&p, Path::new("/anywhere"), 20).unwrap();
    assert_eq!(r.level, PressureLevel::Ok);
    assert_eq!(r.threshold_bytes, 20 * 1024 * 1024 * 1024);
    assert_eq!(r.path, PathBuf::from("/anywhere"));
    assert!(!r.should_refuse());
}

#[test]
fn check_disk_pressure_with_returns_warn_band() {
    // 15 GiB free vs 20 GiB threshold → warn band [10, 20).
    let p = FakeProvider::new(vec![DiskStat {
        free_bytes: 15 * 1024 * 1024 * 1024,
        total_bytes: 100 * 1024 * 1024 * 1024,
    }]);
    let r = check_disk_pressure_with(&p, Path::new("/anywhere"), 20).unwrap();
    assert_eq!(r.level, PressureLevel::Warn);
    assert!(!r.should_refuse());
    let msg = r.warn_message();
    assert!(msg.contains("WARN"), "{msg}");
    assert!(msg.contains("15.0 GiB"), "{msg}");
    assert!(msg.contains("20.0 GiB"), "{msg}");
}

#[test]
fn check_disk_pressure_with_returns_refuse_band() {
    // 5 GiB free vs 20 GiB threshold → refuse (< half).
    let p = FakeProvider::new(vec![DiskStat {
        free_bytes: 5 * 1024 * 1024 * 1024,
        total_bytes: 100 * 1024 * 1024 * 1024,
    }]);
    let r = check_disk_pressure_with(&p, Path::new("/anywhere"), 20).unwrap();
    assert_eq!(r.level, PressureLevel::Refuse);
    assert!(r.should_refuse());

    let msg = r.refuse_message();
    assert!(msg.contains("REFUSED"), "{msg}");
    assert!(msg.contains("5.0 GiB"), "{msg}");
    assert!(msg.contains("20.0 GiB"), "{msg}");
    assert!(msg.contains("/anywhere"), "{msg}");
    assert!(
        msg.contains("simard worktree-gc --apply"),
        "must include remediation hint: {msg}"
    );
}

#[test]
fn refuse_message_contract_is_complete() {
    // The disk-fill incident postmortem requires the refuse message to
    // include: free space, threshold, path, and a remediation hint.
    let r = DiskPressureReport {
        path: PathBuf::from("/srv/state"),
        free_bytes: 1024 * 1024 * 1024,           // 1 GiB
        total_bytes: 100 * 1024 * 1024 * 1024,    // 100 GiB
        threshold_bytes: 20 * 1024 * 1024 * 1024, // 20 GiB
        level: PressureLevel::Refuse,
    };
    let msg = r.refuse_message();
    for needle in [
        "1.0 GiB",
        "/srv/state",
        "20.0 GiB",
        "simard worktree-gc --apply",
    ] {
        assert!(msg.contains(needle), "missing {needle:?} in: {msg}");
    }
}

// ------------------------------------------------------------------
// human_bytes formatter
// ------------------------------------------------------------------

#[test]
fn human_bytes_renders_iec_units() {
    assert_eq!(human_bytes(0), "0 B");
    assert_eq!(human_bytes(512), "512 B");
    assert_eq!(human_bytes(1024), "1.0 KiB");
    assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
    assert_eq!(human_bytes(1024 * 1024 * 1024), "1.0 GiB");
    assert_eq!(human_bytes(20 * 1024 * 1024 * 1024), "20.0 GiB");
}

// ------------------------------------------------------------------
// configured_min_free_gb (env-var parsing)
// ------------------------------------------------------------------

#[test]
#[serial(simard_disk_pressure_env)]
fn configured_min_free_gb_uses_default_when_unset() {
    // SAFETY: tests live in a single process; reset the var around
    // assertions. Use a unique name per test so concurrent tests can
    // be serialized via the shared serial_test crate if needed —
    // here we only read the var so a missing-var path is safe.
    // SAFETY: env var mutation is process-wide; tests in this module
    // may race, but they all touch the same var serially in practice
    // because cargo test is the sole writer.
    unsafe { std::env::remove_var("SIMARD_DISK_PRESSURE_MIN_FREE_GB") };
    assert_eq!(configured_min_free_gb(), DEFAULT_MIN_FREE_GB);
}

#[test]
#[serial(simard_disk_pressure_env)]
fn configured_min_free_gb_parses_valid_value() {
    // SAFETY: env var mutation is process-wide; tests in this module
    // may race, but cargo test is the sole writer.
    unsafe { std::env::set_var("SIMARD_DISK_PRESSURE_MIN_FREE_GB", "5") };
    assert_eq!(configured_min_free_gb(), 5);
    // SAFETY: see above.
    unsafe { std::env::remove_var("SIMARD_DISK_PRESSURE_MIN_FREE_GB") };
}

#[test]
#[serial(simard_disk_pressure_env)]
fn configured_min_free_gb_rejects_zero_and_falls_back() {
    // SAFETY: see above.
    unsafe { std::env::set_var("SIMARD_DISK_PRESSURE_MIN_FREE_GB", "0") };
    assert_eq!(configured_min_free_gb(), DEFAULT_MIN_FREE_GB);
    // SAFETY: see above.
    unsafe { std::env::remove_var("SIMARD_DISK_PRESSURE_MIN_FREE_GB") };
}

#[test]
#[serial(simard_disk_pressure_env)]
fn configured_min_free_gb_rejects_garbage_and_falls_back() {
    // SAFETY: see above.
    unsafe { std::env::set_var("SIMARD_DISK_PRESSURE_MIN_FREE_GB", "twenty") };
    assert_eq!(configured_min_free_gb(), DEFAULT_MIN_FREE_GB);
    // SAFETY: see above.
    unsafe { std::env::remove_var("SIMARD_DISK_PRESSURE_MIN_FREE_GB") };
}

// ------------------------------------------------------------------
// Real provider smoke test (unsandboxed; only verifies syscall succeeds)
// ------------------------------------------------------------------

#[test]
fn real_provider_smoke_test_against_tempdir() {
    // Just validate the syscall doesn't error on a path we know exists.
    // The free-byte value depends on the host disk; we only assert it
    // is non-zero (test runner must have at least 1 byte free).
    let tmp = tempfile::tempdir().expect("tmp");
    let report = check_disk_pressure(tmp.path(), 1).expect("statvfs");
    assert!(report.total_bytes > 0);
    assert!(report.free_bytes > 0);
    assert_eq!(report.threshold_bytes, 1024 * 1024 * 1024);
}
