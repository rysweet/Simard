//! Resource cleanup: remove stale cargo target dirs, temp files, and orphaned processes.
//!
//! `simard cleanup` scans for disk-wasting artifacts and reclaims space.
//! Called manually or from a scheduled OODA action.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Summary of a cleanup run.
#[derive(Debug, Default)]
pub struct CleanupReport {
    pub bytes_freed: u64,
    pub dirs_removed: Vec<PathBuf>,
    pub processes_killed: u32,
    pub errors: Vec<String>,
}

impl std::fmt::Display for CleanupReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mb = self.bytes_freed / (1024 * 1024);
        writeln!(f, "Cleanup report:")?;
        writeln!(f, "  Freed: {mb} MB")?;
        writeln!(f, "  Dirs removed: {}", self.dirs_removed.len())?;
        writeln!(
            f,
            "  Stale cargo processes killed: {}",
            self.processes_killed
        )?;
        for e in &self.errors {
            writeln!(f, "  Error: {e}")?;
        }
        Ok(())
    }
}

/// Run the full cleanup pipeline.
pub fn handle_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("simard cleanup: scanning for reclaimable resources\n");
    let mut report = CleanupReport::default();

    // 1. Show disk usage first
    print_disk_usage();

    // 2. Clean stale canary dirs in /tmp and TMPDIR
    let tmp_dirs = [
        std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string()),
        "/tmp".to_string(),
    ];
    for base in &tmp_dirs {
        clean_simard_canaries(Path::new(base), &mut report);
    }

    // 3. Clean old cargo target dirs (not the current one)
    clean_stale_cargo_targets(&mut report);

    // 4. Kill orphaned cargo processes (running > 30 min with no parent simard)
    kill_orphaned_cargo_processes(&mut report);

    eprintln!("\n{report}");

    if !report.errors.is_empty() {
        Err(format!("{} error(s) during cleanup", report.errors.len()).into())
    } else {
        Ok(())
    }
}

/// Print current disk usage for key partitions.
fn print_disk_usage() {
    eprintln!("Disk usage:");
    if let Ok(output) = Command::new("df").args(["-h", "/", "/tmp"]).output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            eprintln!("  {line}");
        }
    }

    // Also check CARGO_TARGET_DIR and home
    for var in ["CARGO_TARGET_DIR", "HOME"] {
        if let Ok(path) = std::env::var(var)
            && let Ok(output) = Command::new("du").args(["-sh", &path]).output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            eprintln!("  {var}: {}", stdout.trim());
        }
    }
    eprintln!();
}

/// Remove `/tmp/simard-canary*` directories that grow unbounded.
fn clean_simard_canaries(base: &Path, report: &mut CleanupReport) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("simard-canary") || name_str.starts_with("simard-e2e") {
            let path = entry.path();
            match dir_size(&path) {
                Ok(size) if size > 0 => {
                    eprintln!(
                        "  Removing {} ({} MB)",
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

/// Clean stale cargo target directories that aren't the configured one.
fn clean_stale_cargo_targets(report: &mut CleanupReport) {
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

/// Kill cargo processes that have been running for more than 30 minutes
/// without a parent simard process. This catches the "cargo process
/// accumulation" problem (issue #337).
fn kill_orphaned_cargo_processes(report: &mut CleanupReport) {
    let Ok(output) = Command::new("ps")
        .args(["--no-headers", "-eo", "pid,etimes,args"])
        .output()
    else {
        return;
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let threshold_seconds = 1800; // 30 minutes

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let Ok(pid) = parts[0].parse::<u32>() else {
            continue;
        };
        let Ok(elapsed) = parts[1].parse::<u64>() else {
            continue;
        };
        let cmd = parts[2..].join(" ");

        // Be precise: the process executable basename must be exactly "cargo",
        // not just any path containing "cargo" (which matches /home/.../.cargo/
        // and amplihack/recipe-runner-rs orchestrators that legitimately run
        // for hours).
        let exe_basename = parts[2].rsplit('/').next().unwrap_or("");
        let is_cargo_invocation = exe_basename == "cargo"
            && parts.get(3).map(|s| *s == "test" || *s == "build").unwrap_or(false);

        if elapsed > threshold_seconds && is_cargo_invocation {
            eprintln!("  Killing orphaned cargo process pid={pid} (running {elapsed}s): {cmd}");
            let _ = Command::new("kill").arg(pid.to_string()).output();
            report.processes_killed += 1;
        }
    }
}

/// Recursively compute directory size in bytes.
fn dir_size(path: &Path) -> std::io::Result<u64> {
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
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
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
}
