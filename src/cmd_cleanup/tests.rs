use super::*;

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
