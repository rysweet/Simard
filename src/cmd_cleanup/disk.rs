//! Disk-cleanup helpers extracted from mod.rs (#1266).

use std::path::{Path, PathBuf};

use super::CleanupReport;

pub fn clean_simard_canaries(base: &Path, report: &mut CleanupReport) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    // Anything older than this is fair game. 1 day is conservative — a real
    // build would never linger in /tmp that long.
    let max_age = std::time::Duration::from_secs(24 * 3600);
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Original canary patterns plus the broader simard-/amplihack-/ia2-
        // session artifacts that accumulate from interactive sessions and
        // detached recipe runners.
        let matches_pattern = name_str.starts_with("simard-canary")
            || name_str.starts_with("simard-e2e")
            || name_str.starts_with("simard-")
            || name_str.starts_with("amplihack-")
            || name_str.starts_with("amplihack_eval")
            || name_str.starts_with("ia2-");
        if !matches_pattern {
            continue;
        }
        // Never touch the active build target directory.
        let path = entry.path();
        if let Ok(current) = std::env::var("CARGO_TARGET_DIR")
            && path.as_os_str() == current.as_str()
        {
            continue;
        }
        // Age check — leave fresh artifacts alone.
        if let Ok(meta) = entry.metadata()
            && let Ok(modified) = meta.modified()
            && now.duration_since(modified).unwrap_or_default() < max_age
        {
            continue;
        }
        match dir_size(&path) {
            Ok(size) if size > 0 => {
                eprintln!(
                    "  Removing {} ({} MB)",
                    path.display(),
                    size / (1024 * 1024)
                );
                let removed = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                if let Err(e) = removed {
                    report
                        .errors
                        .push(format!("failed to remove {}: {e}", path.display()));
                } else {
                    report.bytes_freed += size;
                    report.dirs_removed.push(path);
                }
            }
            _ => {}
        }
    }
}

/// Clean stale cargo target directories that aren't the configured one.
pub fn clean_stale_cargo_targets(report: &mut CleanupReport) {
    let current_target = std::env::var("CARGO_TARGET_DIR").ok();
    let candidates = ["/tmp/simard-canary", "/tmp/cargo-target"];

    for candidate in &candidates {
        let path = PathBuf::from(candidate);
        if Some(candidate.to_string()) == current_target {
            continue;
        }
        if path.exists() && path.is_dir() {
            match dir_size(&path) {
                Ok(size) if size > 10 * 1024 * 1024 => {
                    eprintln!(
                        "  Removing stale target {} ({} MB)",
                        path.display(),
                        size / (1024 * 1024)
                    );
                    if let Err(e) = std::fs::remove_dir_all(&path) {
                        report
                            .errors
                            .push(format!("failed to remove {}: {e}", path.display()));
                    } else {
                        report.bytes_freed += size;
                        report.dirs_removed.push(path);
                    }
                }
                _ => {}
            }
        }
    }
}

/// LRU-rotate `/tmp/simard-*-target` directories when total size exceeds
/// `cap_bytes`. Keeps the currently-configured `CARGO_TARGET_DIR` and the
/// freshest dirs by mtime; removes the oldest until total is under
/// `cap_bytes * 8/10` (so we don't rotate again on the very next call).
///
/// Why: `/tmp/simard-engineer-target` and `/tmp/simard-pr*-target` were
/// observed at 14-15 GB each on 2026-04-25, uncapped. Disk pressure
/// caused multiple OOM linker failures and forced `--no-verify` pushes.
/// (Issue #1244.)
pub fn cap_simard_target_dirs(report: &mut CleanupReport, cap_bytes: u64) {
    let current_target = std::env::var("CARGO_TARGET_DIR").ok();
    let tmp = Path::new("/tmp");
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return;
    };
    // Collect candidates: /tmp/simard-*-target dirs with their size+mtime.
    let mut candidates: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !(name.starts_with("simard-") && name.ends_with("-target")) {
            continue;
        }
        // Never delete the active CARGO_TARGET_DIR.
        if let Some(ref current) = current_target
            && Path::new(current) == path
        {
            continue;
        }
        let size = dir_size(&path).unwrap_or(0);
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        candidates.push((path, size, mtime));
    }
    let total: u64 = candidates.iter().map(|(_, s, _)| s).sum();
    if total <= cap_bytes {
        return;
    }
    eprintln!(
        "  /tmp/simard-*-target total {} MB exceeds cap {} MB — rotating LRU",
        total / (1024 * 1024),
        cap_bytes / (1024 * 1024)
    );
    // Sort oldest-first; remove until under 80% of cap.
    candidates.sort_by_key(|(_, _, mtime)| *mtime);
    let target_after = cap_bytes * 8 / 10;
    let mut current_total = total;
    for (path, size, _mtime) in candidates {
        if current_total <= target_after {
            break;
        }
        eprintln!(
            "  Rotating LRU target {} ({} MB)",
            path.display(),
            size / (1024 * 1024)
        );
        if let Err(e) = std::fs::remove_dir_all(&path) {
            report
                .errors
                .push(format!("failed to rotate {}: {e}", path.display()));
        } else {
            report.bytes_freed += size;
            current_total = current_total.saturating_sub(size);
            report.dirs_removed.push(path);
        }
    }
}

/// Kill cargo processes that have been running for more than 30 minutes
/// without a parent simard process. This catches the "cargo process
/// accumulation" problem (issue #337).
pub fn dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                total += dir_size(&entry.path()).unwrap_or(0);
            } else {
                total += metadata.len();
            }
        }
    } else if path.is_file() {
        total = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    Ok(total)
}

/// Number of binary backups to keep in `~/.simard/bin/`.
/// Each binary is ~34 MB; keeping 2 is enough to roll back one bad deploy.
pub const BINARY_BACKUPS_KEEP: usize = 2;

/// Rotate `~/.simard/bin/simard.bak-*`, keeping only the newest N.
/// Each deploy creates a new backup and they accumulate without bound;
/// in practice they reach 1+ GB if you also keep an old debug build around.
pub fn rotate_simard_binary_backups(report: &mut CleanupReport) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let bin_dir = PathBuf::from(home).join(".simard").join("bin");
    let Ok(entries) = std::fs::read_dir(&bin_dir) else {
        return;
    };
    let mut backups: Vec<(PathBuf, std::time::SystemTime)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("simard.bak-") {
                return None;
            }
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((e.path(), modified))
        })
        .collect();
    if backups.len() <= BINARY_BACKUPS_KEEP {
        return;
    }
    // Newest first.
    backups.sort_by_key(|b| std::cmp::Reverse(b.1));
    for (path, _) in backups.into_iter().skip(BINARY_BACKUPS_KEEP) {
        let size = dir_size(&path).unwrap_or(0);
        eprintln!(
            "  Rotating binary backup {} ({} MB)",
            path.display(),
            size / (1024 * 1024)
        );
        if let Err(e) = std::fs::remove_file(&path) {
            report
                .errors
                .push(format!("failed to rotate {}: {e}", path.display()));
        } else {
            report.bytes_freed += size;
            report.dirs_removed.push(path);
        }
    }
}

/// LRU-rotate `~/.cargo-targets/` subdirectories when total size exceeds
/// `cap_bytes`. Each engineer worktree gets a per-worktree subdirectory here
/// (set by `DEFAULT_CARGO_TARGETS_HOME_SUBDIR` in `agent_supervisor/tmux.rs`).
/// Without capping, these accumulate at ~8-16 GB each and can fill the home
/// partition entirely (observed 2026-05-29: 131 GB across 15 worktrees, 0
/// bytes available on /home).
pub fn cap_home_cargo_targets(report: &mut CleanupReport, cap_bytes: u64) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let targets_root = PathBuf::from(home).join(".cargo-targets");
    let Ok(entries) = std::fs::read_dir(&targets_root) else {
        return;
    };
    let current_target = std::env::var("CARGO_TARGET_DIR").ok();
    let mut candidates: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Never delete the active CARGO_TARGET_DIR.
        if let Some(ref current) = current_target
            && Path::new(current) == path
        {
            continue;
        }
        let size = dir_size(&path).unwrap_or(0);
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        candidates.push((path, size, mtime));
    }
    let total: u64 = candidates.iter().map(|(_, s, _)| s).sum();
    if total <= cap_bytes {
        return;
    }
    eprintln!(
        "  ~/.cargo-targets/ total {} MB exceeds cap {} MB — rotating LRU",
        total / (1024 * 1024),
        cap_bytes / (1024 * 1024)
    );
    // Sort oldest-first; remove until under 80% of cap.
    candidates.sort_by_key(|(_, _, mtime)| *mtime);
    let target_after = cap_bytes * 8 / 10;
    let mut current_total = total;
    for (path, size, _mtime) in candidates {
        if current_total <= target_after {
            break;
        }
        eprintln!(
            "  Rotating LRU home target {} ({} MB)",
            path.display(),
            size / (1024 * 1024)
        );
        if let Err(e) = std::fs::remove_dir_all(&path) {
            report
                .errors
                .push(format!("failed to rotate {}: {e}", path.display()));
        } else {
            report.bytes_freed += size;
            current_total = current_total.saturating_sub(size);
            report.dirs_removed.push(path);
        }
    }
}

/// Maximum age (in days) of corrupted memory DB files before deletion.
pub const CORRUPT_DB_MAX_AGE_DAYS: u64 = 7;

/// Maximum number of quarantined corrupt cognitive-memory artifacts to retain,
/// regardless of age (issue #2420).
///
/// The age cap alone ([`CORRUPT_DB_MAX_AGE_DAYS`]) is unbounded *within* the
/// window: a burst of WAL-corruption events (this host saw 88 MB / 112 artifacts
/// accumulate) leaves dozens of fresh `cognitive*.corrupt-*` / `*.shadow`
/// quarantines that the age rule will not touch for a week. Keeping only the
/// newest N — in addition to the age cap — bounds that growth so the most recent
/// forensic snapshots survive while older ones are reclaimed immediately. Mirrors
/// the keep-last-N retention already used for [`BINARY_BACKUPS_KEEP`] /
/// [`SNAPSHOTS_KEEP`].
pub const CORRUPT_DB_KEEP: usize = 5;

/// Returns `true` when `name` is a quarantined corrupt cognitive-memory
/// artifact (safe to garbage-collect) and `false` for the live store.
///
/// Two backend generations leave quarantine files in `~/.simard`:
///
/// * **Native fork (pre-#2307).** A single-file store at
///   `cognitive_memory.ladybug`; corrupt snapshots were renamed
///   `cognitive_memory.corrupt-<ts>`.
/// * **Library backend (de-fork Phase 2b, #2307).** The store lives at
///   `cognitive` with a `cognitive.wal` write-ahead log. When LadybugDB
///   detects corruption it quarantines the bad file in place, producing
///   `cognitive.corrupt-<ts>`, `cognitive.wal.corrupt-<ts>`, recursively
///   nested `…cognitive.wal.corrupt-<ts>` chains, `…cognitive.shadow`
///   side-files, and `…corrupt-<ts>.bak` copies.
///
/// Every quarantine name carries the `.corrupt-<ts>` infix, while the live
/// store files (`cognitive`, `cognitive.wal`, `cognitive.shadow`) never do.
/// Matching on that infix — anchored to the `cognitive`/`cognitive_memory`
/// store stem — therefore reclaims every stale quarantine generation without
/// ever risking the live store. Before this generalization the cleanup only
/// matched the native `cognitive_memory.corrupt-` prefix, so post-de-fork
/// library quarantines accumulated unbounded (issue #2307 fallout).
pub(crate) fn is_corrupt_quarantine_name(name: &str) -> bool {
    (name.starts_with("cognitive.") || name.starts_with("cognitive_memory."))
        && name.contains(".corrupt-")
}

/// Remove quarantined corrupt cognitive-memory snapshots in `~/.simard` that are
/// either older than [`CORRUPT_DB_MAX_AGE_DAYS`] **or** beyond the newest
/// [`CORRUPT_DB_KEEP`] quarantines (issue #2420). Covers both the native
/// single-file `cognitive_memory.corrupt-*` snapshots and the library backend's
/// `cognitive.corrupt-*` / `cognitive.wal.corrupt-*` / `*.cognitive.shadow`
/// quarantines (see [`is_corrupt_quarantine_name`]). These are useful briefly
/// for forensics then pure dead weight. The live store (`cognitive`,
/// `cognitive.wal`) is never matched.
///
/// The age cap alone leaves a burst of *young* quarantines untouched for a week
/// (this host saw 88 MB / 112 artifacts accumulate); the keep-last-N cap bounds
/// that growth immediately while preserving the most recent forensic snapshots.
pub fn remove_old_corrupt_dbs(report: &mut CleanupReport) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let simard_dir = PathBuf::from(home).join(".simard");
    let Ok(entries) = std::fs::read_dir(&simard_dir) else {
        return;
    };
    let max_age = std::time::Duration::from_secs(CORRUPT_DB_MAX_AGE_DAYS * 24 * 3600);
    let now = std::time::SystemTime::now();

    // Collect every quarantine candidate with its mtime so we can apply both the
    // age cap and the keep-last-N cap over the full set (read_dir order is
    // unspecified, so we cannot rank by iteration order).
    let mut candidates: Vec<(PathBuf, std::fs::Metadata, std::time::SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_corrupt_quarantine_name(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        candidates.push((entry.path(), meta, modified));
    }

    // Newest first, so index 0..CORRUPT_DB_KEEP are the survivors of the count cap.
    candidates.sort_by_key(|c| std::cmp::Reverse(c.2));

    for (rank, (path, meta, modified)) in candidates.iter().enumerate() {
        let too_old = now.duration_since(*modified).unwrap_or_default() >= max_age;
        let beyond_keep = rank >= CORRUPT_DB_KEEP;
        if !too_old && !beyond_keep {
            continue;
        }
        // A cognitive-memory store may be backed by a single file or a
        // directory, so a quarantine snapshot can be either. Size and remove
        // accordingly; `remove_file` on a directory would otherwise fail and
        // leave the quarantine in place.
        let is_dir = meta.is_dir();
        let size = if is_dir {
            dir_size(path).unwrap_or(0)
        } else {
            meta.len()
        };
        let reason = if too_old { "age" } else { "keep-last-N" };
        eprintln!(
            "  Removing corrupt DB {} ({} MB, {reason})",
            path.display(),
            size / (1024 * 1024)
        );
        let removed = if is_dir {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        if let Err(e) = removed {
            report
                .errors
                .push(format!("failed to remove {}: {e}", path.display()));
        } else {
            report.bytes_freed += size;
            report.dirs_removed.push(path.clone());
        }
    }
}

/// Maximum number of memory snapshot files to retain.
/// One snapshot is written per OODA cycle; with a 5-minute interval, 100 files
/// is roughly 8 hours of recent state — plenty for incident review.
pub const SNAPSHOTS_KEEP: usize = 100;

/// Trim `~/.simard/snapshots/session-*.json`, keeping only the newest N.
/// These accumulate at one per cycle indefinitely.
pub fn trim_simard_snapshots(report: &mut CleanupReport) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let snap_dir = PathBuf::from(home).join(".simard").join("snapshots");
    let Ok(entries) = std::fs::read_dir(&snap_dir) else {
        return;
    };
    let mut snaps: Vec<(PathBuf, std::time::SystemTime, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("session-") || !name.ends_with(".json") {
                return None;
            }
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?;
            Some((e.path(), modified, meta.len()))
        })
        .collect();
    if snaps.len() <= SNAPSHOTS_KEEP {
        return;
    }
    snaps.sort_by_key(|b| std::cmp::Reverse(b.1));
    for (path, _, size) in snaps.into_iter().skip(SNAPSHOTS_KEEP) {
        if let Err(e) = std::fs::remove_file(&path) {
            report
                .errors
                .push(format!("failed to trim {}: {e}", path.display()));
        } else {
            report.bytes_freed += size;
            report.dirs_removed.push(path);
        }
    }
}
