use super::*;

/// Set a path's mtime to roughly `days` days in the past (plus an hour of
/// slack so age comparisons are unambiguous). Works for both files and
/// directories on Unix via a read-only handle owned by the test.
fn set_mtime_days_ago(path: &Path, days: u64) {
    let mtime =
        std::time::SystemTime::now() - std::time::Duration::from_secs(days * 24 * 3600 + 3600);
    let times = std::fs::FileTimes::new().set_modified(mtime);
    std::fs::File::open(path).unwrap().set_times(times).unwrap();
}

/// Restore a previously-captured environment variable to its prior value
/// (or remove it if it was originally unset).
fn restore_env(key: &str, prev: Option<std::ffi::OsString>) {
    // SAFETY: callers run under `#[serial_test::serial]`, so no other thread
    // touches the process environment concurrently.
    unsafe {
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}

#[test]
fn cleanup_report_display_includes_stats() {
    let report = CleanupReport {
        bytes_freed: 1024 * 1024 * 500,
        dirs_removed: vec![PathBuf::from("/tmp/simard-canary")],
        processes_killed: 2,
        errors: vec!["test error".to_string()],
    };
    let s = report.to_string();
    assert!(s.contains("500 MB"), "should show MB: {s}");
    assert!(s.contains("1"), "should count dirs: {s}");
    assert!(s.contains("2"), "should count processes: {s}");
    assert!(s.contains("test error"), "should show errors: {s}");
}

#[test]
fn cleanup_report_default_is_empty() {
    let report = CleanupReport::default();
    assert_eq!(report.bytes_freed, 0);
    assert!(report.dirs_removed.is_empty());
    assert_eq!(report.processes_killed, 0);
    assert!(report.errors.is_empty());
}

#[test]
fn dir_size_empty_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let size = dir_size(tmp.path()).unwrap();
    assert_eq!(size, 0);
}

#[test]
fn dir_size_with_files() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "hello").unwrap();
    std::fs::write(tmp.path().join("b.txt"), "world!").unwrap();
    let size = dir_size(tmp.path()).unwrap();
    assert_eq!(size, 11); // "hello" (5) + "world!" (6)
}

#[test]
fn disk_usage_does_not_panic() {
    // Just verifying it doesn't crash
    print_disk_usage();
}

// ── cap_simard_target_dirs (P4 / #1244) ──

#[test]
fn cap_simard_target_dirs_under_cap_is_noop() {
    // We can't easily redirect the function's hardcoded /tmp scan in a
    // unit test, so we run with a cap so high it's guaranteed under it
    // on a normal test host and assert nothing was rotated.
    let mut report = CleanupReport::default();
    cap_simard_target_dirs(&mut report, u64::MAX);
    assert_eq!(report.bytes_freed, 0);
    assert!(report.dirs_removed.is_empty());
    assert!(report.errors.is_empty());
}

#[test]
fn cap_simard_target_dirs_lru_rotation_logic() {
    // Direct-test the size accounting and ordering invariant via the
    // helper structures. We synthesise a fake /tmp.
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    // Three fake target dirs of different sizes and ages. Sleep
    // between creates so mtimes are distinct (we can't use filetime
    // without adding a new dep, and `std::fs::set_modified` requires
    // touching the file — easier to just sleep).
    let make = |name: &str, bytes: usize| {
        let p = base.join(name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("payload"), vec![0u8; bytes]).unwrap();
        p
    };
    let d_old = make("simard-old-target", 6 * 1024 * 1024);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let d_mid = make("simard-mid-target", 6 * 1024 * 1024);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let d_new = make("simard-new-target", 6 * 1024 * 1024);

    // Manually replicate the candidate-collect+sort+rotate loop to
    // verify the algorithm: cap is 10 MB, total is 18 MB, so we should
    // rotate the oldest (6 MB) leaving 12 MB > 8 MB target. Then rotate
    // the next oldest (mid, 6 MB) leaving 6 MB ≤ 8 MB target. Stop.
    let cap_bytes: u64 = 10 * 1024 * 1024;
    let mut candidates: Vec<(PathBuf, u64, std::time::SystemTime)> = vec![
        (
            d_old.clone(),
            dir_size(&d_old).unwrap(),
            std::fs::metadata(&d_old).unwrap().modified().unwrap(),
        ),
        (
            d_mid.clone(),
            dir_size(&d_mid).unwrap(),
            std::fs::metadata(&d_mid).unwrap().modified().unwrap(),
        ),
        (
            d_new.clone(),
            dir_size(&d_new).unwrap(),
            std::fs::metadata(&d_new).unwrap().modified().unwrap(),
        ),
    ];
    candidates.sort_by_key(|(_, _, mtime)| *mtime);
    // Oldest first.
    assert_eq!(candidates[0].0, d_old);
    assert_eq!(candidates[2].0, d_new);

    let total: u64 = candidates.iter().map(|(_, s, _)| s).sum();
    let target_after = cap_bytes * 8 / 10;
    let mut current_total = total;
    let mut rotated = Vec::new();
    for (path, size, _) in candidates {
        if current_total <= target_after {
            break;
        }
        current_total = current_total.saturating_sub(size);
        rotated.push(path);
    }
    // Expected: rotate the two oldest (d_old and d_mid), keep d_new.
    assert_eq!(rotated.len(), 2);
    assert!(rotated.contains(&d_old));
    assert!(rotated.contains(&d_mid));
    assert!(!rotated.contains(&d_new));
}

// ── Constant sanity ──

#[test]
fn binary_backups_keep_at_least_one() {
    // At least one backup must always be retained — losing the rollback
    // option silently is worse than the disk savings.
    const { assert!(BINARY_BACKUPS_KEEP >= 1) };
}

#[test]
fn snapshot_retention_covers_at_least_an_hour() {
    // With the default 5-min OODA cycle, 12 snapshots = 1 hour.
    const { assert!(SNAPSHOTS_KEEP >= 12) };
}

#[test]
fn corrupt_db_retention_at_least_a_day() {
    const { assert!(CORRUPT_DB_MAX_AGE_DAYS >= 1) };
}

// ── rotate_simard_binary_backups ──

#[test]
#[serial_test::serial]
fn rotate_keeps_newest_n_backups() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join(".simard").join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // Create 5 fake backup files with progressively newer mtimes.
    for i in 0..5 {
        let p = bin_dir.join(format!("simard.bak-{i}"));
        std::fs::write(&p, vec![0u8; 1024]).unwrap();
        // Set mtime via filetime so they sort deterministically.
        let mtime = std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(1_000_000_000 + (i as u64) * 1000);
        let times = std::fs::FileTimes::new().set_modified(mtime);
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_times(times)
            .unwrap();
    }
    // Override HOME so the function targets our tempdir.
    let old_home = std::env::var_os("HOME");
    // SAFETY: test is single-threaded for env access; restored below.
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let mut report = CleanupReport::default();
    rotate_simard_binary_backups(&mut report);
    if let Some(h) = old_home {
        unsafe {
            std::env::set_var("HOME", h);
        }
    }
    let remaining: Vec<_> = std::fs::read_dir(&bin_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        remaining.len(),
        BINARY_BACKUPS_KEEP,
        "should keep exactly {BINARY_BACKUPS_KEEP}: {remaining:?}"
    );
    // The two newest (4 and 3) should survive.
    assert!(remaining.iter().any(|n| n.ends_with("-4")));
    assert!(remaining.iter().any(|n| n.ends_with("-3")));
    assert_eq!(report.dirs_removed.len(), 3);
}

#[test]
#[serial_test::serial]
fn rotate_noop_when_under_threshold() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join(".simard").join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(bin_dir.join("simard.bak-only"), b"x").unwrap();
    let old_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let mut report = CleanupReport::default();
    rotate_simard_binary_backups(&mut report);
    if let Some(h) = old_home {
        unsafe {
            std::env::set_var("HOME", h);
        }
    }
    assert!(bin_dir.join("simard.bak-only").exists());
    assert_eq!(report.dirs_removed.len(), 0);
}

// ── trim_simard_snapshots ──

#[test]
#[serial_test::serial]
fn trim_snapshots_keeps_newest_n() {
    let tmp = tempfile::tempdir().unwrap();
    let snap_dir = tmp.path().join(".simard").join("snapshots");
    std::fs::create_dir_all(&snap_dir).unwrap();
    // Write SNAPSHOTS_KEEP + 5 files
    let n = SNAPSHOTS_KEEP + 5;
    for i in 0..n {
        let p = snap_dir.join(format!("session-{i:04}.json"));
        std::fs::write(&p, b"{}").unwrap();
        let mtime =
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000 + i as u64);
        let times = std::fs::FileTimes::new().set_modified(mtime);
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_times(times)
            .unwrap();
    }
    let old_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let mut report = CleanupReport::default();
    trim_simard_snapshots(&mut report);
    if let Some(h) = old_home {
        unsafe {
            std::env::set_var("HOME", h);
        }
    }
    let remaining = std::fs::read_dir(&snap_dir).unwrap().count();
    assert_eq!(remaining, SNAPSHOTS_KEEP);
    assert_eq!(report.dirs_removed.len(), 5);
}

// ── remove_old_corrupt_dbs ──

#[test]
#[serial_test::serial]
fn corrupt_db_removed_when_older_than_threshold() {
    let tmp = tempfile::tempdir().unwrap();
    let simard = tmp.path().join(".simard");
    std::fs::create_dir_all(&simard).unwrap();
    let old = simard.join("cognitive_memory.corrupt-old");
    let young = simard.join("cognitive_memory.corrupt-young");
    let unrelated = simard.join("cognitive_memory.ladybug");
    std::fs::write(&old, b"old").unwrap();
    std::fs::write(&young, b"young").unwrap();
    std::fs::write(&unrelated, b"keep").unwrap();
    let old_mtime = std::time::SystemTime::now()
        - std::time::Duration::from_secs((CORRUPT_DB_MAX_AGE_DAYS + 1) * 24 * 3600);
    let times = std::fs::FileTimes::new().set_modified(old_mtime);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_times(times)
        .unwrap();
    let old_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let mut report = CleanupReport::default();
    remove_old_corrupt_dbs(&mut report);
    if let Some(h) = old_home {
        unsafe {
            std::env::set_var("HOME", h);
        }
    }
    assert!(!old.exists(), "old corrupt DB should be removed");
    assert!(young.exists(), "young corrupt DB should survive");
    assert!(unrelated.exists(), "non-corrupt DB must never be touched");
}

// ── clean_simard_canaries ──

#[test]
#[serial_test::serial]
fn clean_simard_canaries_removes_old_matching_only() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    // Old + matching directory with content -> removed via remove_dir_all.
    let old_dir = base.join("simard-canary-old");
    std::fs::create_dir_all(&old_dir).unwrap();
    std::fs::write(old_dir.join("payload"), vec![0u8; 2048]).unwrap();
    set_mtime_days_ago(&old_dir, 3);

    // Old + matching file with content -> removed via remove_file.
    let old_file = base.join("amplihack-old.bin");
    std::fs::write(&old_file, vec![0u8; 4096]).unwrap();
    set_mtime_days_ago(&old_file, 3);

    // Old + matching but empty -> the `size > 0` guard leaves it in place.
    let empty = base.join("ia2-empty");
    std::fs::write(&empty, b"").unwrap();
    set_mtime_days_ago(&empty, 3);

    // Old + matching but pointed at by CARGO_TARGET_DIR -> never touched.
    let protected = base.join("simard-protected");
    std::fs::create_dir_all(&protected).unwrap();
    std::fs::write(protected.join("payload"), vec![0u8; 2048]).unwrap();
    set_mtime_days_ago(&protected, 3);

    // Fresh + matching -> the age check keeps it.
    let young = base.join("amplihack-young");
    std::fs::create_dir_all(&young).unwrap();
    std::fs::write(young.join("payload"), vec![0u8; 2048]).unwrap();

    // Old but non-matching name -> ignored.
    let unrelated = base.join("unrelated-old.bin");
    std::fs::write(&unrelated, vec![0u8; 2048]).unwrap();
    set_mtime_days_ago(&unrelated, 3);

    let prev = std::env::var_os("CARGO_TARGET_DIR");
    // SAFETY: serialized via serial_test; restored immediately after the call.
    unsafe {
        std::env::set_var("CARGO_TARGET_DIR", &protected);
    }
    let mut report = CleanupReport::default();
    clean_simard_canaries(base, &mut report);
    restore_env("CARGO_TARGET_DIR", prev);

    assert!(!old_dir.exists(), "old matching dir should be removed");
    assert!(!old_file.exists(), "old matching file should be removed");
    assert!(empty.exists(), "zero-byte match must be left in place");
    assert!(protected.exists(), "CARGO_TARGET_DIR must never be removed");
    assert!(young.exists(), "fresh artifacts must be left alone");
    assert!(unrelated.exists(), "non-matching names must be ignored");

    assert_eq!(
        report.dirs_removed.len(),
        2,
        "exactly two artifacts removed"
    );
    assert!(report.dirs_removed.contains(&old_dir));
    assert!(report.dirs_removed.contains(&old_file));
    assert_eq!(report.bytes_freed, 2048 + 4096);
    assert!(report.errors.is_empty());
}

#[test]
fn clean_simard_canaries_missing_base_is_noop() {
    let mut report = CleanupReport::default();
    // read_dir on a non-existent path returns Err -> early return.
    clean_simard_canaries(
        Path::new("/nonexistent-simard-canary-xyz-987654"),
        &mut report,
    );
    assert_eq!(report.bytes_freed, 0);
    assert!(report.dirs_removed.is_empty());
    assert!(report.errors.is_empty());
}

#[test]
#[serial_test::serial]
fn clean_simard_canaries_records_error_when_removal_fails() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("scan");
    std::fs::create_dir_all(&base).unwrap();
    let old_dir = base.join("simard-stuck");
    std::fs::create_dir_all(&old_dir).unwrap();
    std::fs::write(old_dir.join("payload"), vec![0u8; 2048]).unwrap();
    set_mtime_days_ago(&old_dir, 3);

    // A read-only parent makes unlinking the matched child fail for a
    // non-root user, exercising the error-recording branch.
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o555)).unwrap();

    let prev = std::env::var_os("CARGO_TARGET_DIR");
    // SAFETY: serialized via serial_test; restored immediately after the call.
    unsafe {
        std::env::remove_var("CARGO_TARGET_DIR");
    }
    let mut report = CleanupReport::default();
    clean_simard_canaries(&base, &mut report);
    restore_env("CARGO_TARGET_DIR", prev);

    // Restore write permission so the tempdir can be cleaned up.
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();

    if report.errors.is_empty() {
        // Suite running as root bypasses the read-only bit; removal succeeds.
        assert!(!old_dir.exists());
    } else {
        assert!(old_dir.exists(), "removal must have failed");
        assert_eq!(report.bytes_freed, 0);
        assert!(report.dirs_removed.is_empty());
    }
}

// ── clean_stale_cargo_targets ──

#[test]
#[serial_test::serial]
fn clean_stale_cargo_targets_skips_configured_target() {
    // Point CARGO_TARGET_DIR at one hard-coded candidate so the function
    // `continue`s past it; the other candidate ("/tmp/cargo-target") is
    // absent on the test host, exercising the existence check. Nothing real
    // is deleted.
    let prev = std::env::var_os("CARGO_TARGET_DIR");
    // SAFETY: serialized via serial_test; restored immediately after the call.
    unsafe {
        std::env::set_var("CARGO_TARGET_DIR", "/tmp/simard-canary");
    }
    let mut report = CleanupReport::default();
    clean_stale_cargo_targets(&mut report);
    restore_env("CARGO_TARGET_DIR", prev);

    assert!(report.errors.is_empty());
}

// ── cap_home_cargo_targets ──

#[test]
#[serial_test::serial]
fn cap_home_cargo_targets_rotates_lru_over_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".cargo-targets");
    std::fs::create_dir_all(&root).unwrap();

    let make = |name: &str, days: u64| -> PathBuf {
        let p = root.join(name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("payload"), vec![0u8; 6 * 1024 * 1024]).unwrap();
        set_mtime_days_ago(&p, days);
        p
    };
    let oldest = make("wt-oldest", 3);
    let middle = make("wt-middle", 2);
    let newest = make("wt-newest", 1);

    let prev = std::env::var_os("HOME");
    // SAFETY: serialized via serial_test; restored immediately after the call.
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let mut report = CleanupReport::default();
    // Total 18 MB, cap 10 MB, drain target is 8 MB: drop oldest (->12 MB),
    // then middle (->6 MB <= 8 MB) and stop, keeping the newest.
    cap_home_cargo_targets(&mut report, 10 * 1024 * 1024);
    restore_env("HOME", prev);

    assert!(!oldest.exists(), "oldest target should be rotated out");
    assert!(!middle.exists(), "middle target should be rotated out");
    assert!(newest.exists(), "newest target must be kept");
    assert_eq!(report.dirs_removed.len(), 2);
    assert!(report.bytes_freed >= 12 * 1024 * 1024);
    assert!(report.errors.is_empty());
}

#[test]
#[serial_test::serial]
fn cap_home_cargo_targets_under_cap_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".cargo-targets");
    std::fs::create_dir_all(&root).unwrap();
    let p = root.join("wt-small");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("payload"), vec![0u8; 1024]).unwrap();

    let prev = std::env::var_os("HOME");
    // SAFETY: serialized via serial_test; restored immediately after the call.
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let mut report = CleanupReport::default();
    cap_home_cargo_targets(&mut report, u64::MAX);
    restore_env("HOME", prev);

    assert!(p.exists(), "nothing should be rotated when under cap");
    assert_eq!(report.dirs_removed.len(), 0);
    assert_eq!(report.bytes_freed, 0);
    assert!(report.errors.is_empty());
}

#[test]
#[serial_test::serial]
fn cap_home_cargo_targets_missing_root_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    // No ~/.cargo-targets directory -> read_dir Err -> early return.
    let prev = std::env::var_os("HOME");
    // SAFETY: serialized via serial_test; restored immediately after the call.
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let mut report = CleanupReport::default();
    cap_home_cargo_targets(&mut report, 1024);
    restore_env("HOME", prev);

    assert_eq!(report.dirs_removed.len(), 0);
    assert!(report.errors.is_empty());
}

#[test]
#[serial_test::serial]
fn cap_home_cargo_targets_records_error_on_unremovable_target() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".cargo-targets");
    std::fs::create_dir_all(&root).unwrap();

    let big = root.join("wt-big");
    std::fs::create_dir_all(&big).unwrap();
    std::fs::write(big.join("payload"), vec![0u8; 6 * 1024 * 1024]).unwrap();
    set_mtime_days_ago(&big, 3);
    let small = root.join("wt-small");
    std::fs::create_dir_all(&small).unwrap();
    std::fs::write(small.join("payload"), vec![0u8; 1024 * 1024]).unwrap();
    set_mtime_days_ago(&small, 1);

    // Read-only parent makes the rotation's remove_dir_all fail for a
    // non-root user, exercising the error-recording branch.
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();

    let prev = std::env::var_os("HOME");
    // SAFETY: serialized via serial_test; restored immediately after the call.
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }
    let mut report = CleanupReport::default();
    // 7 MB total, cap 5 MB, drain target 4 MB: only the 6 MB oldest is rotated.
    cap_home_cargo_targets(&mut report, 5 * 1024 * 1024);
    restore_env("HOME", prev);

    // Restore write permission so the tempdir can be cleaned up.
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

    if report.errors.is_empty() {
        // Suite running as root bypasses the read-only bit; removal succeeds.
        assert!(!big.exists());
    } else {
        assert!(big.exists(), "removal must have failed");
        assert_eq!(report.bytes_freed, 0);
        assert!(report.dirs_removed.is_empty());
    }
}

// ── kill_orphaned_cargo_processes ──

#[test]
fn kill_orphaned_cargo_processes_scans_cleanly() {
    // No `cargo test`/`cargo build` has been running for over 30 minutes
    // during this suite, so the scan kills nothing. We only assert it
    // completes without recording an error.
    let mut report = CleanupReport::default();
    kill_orphaned_cargo_processes(&mut report);
    assert!(report.errors.is_empty());
}
