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

    // 3b. LRU-rotate /tmp/simard-*-target dirs over the cap (P4 / #1244).
    cap_simard_target_dirs(&mut report, 10 * 1024 * 1024 * 1024);

    // 4. Kill orphaned cargo processes (running > 30 min with no parent simard)
    kill_orphaned_cargo_processes(&mut report);

    // 5. Rotate ~/.simard/bin/simard.bak-* keeping newest N
    rotate_simard_binary_backups(&mut report);

    // 6. Remove old corrupted memory DBs
    remove_old_corrupt_dbs(&mut report);

    // 7. Trim ~/.simard/snapshots/ keeping newest N
    trim_simard_snapshots(&mut report);

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
            && parts
                .get(3)
                .map(|s| *s == "test" || *s == "build")
                .unwrap_or(false);

        if elapsed > threshold_seconds && is_cargo_invocation {
            eprintln!("  Killing orphaned cargo process pid={pid} (running {elapsed}s): {cmd}");
            let _ = Command::new("kill").arg(pid.to_string()).output();
            report.processes_killed += 1;
        }
    }
}

/// Recursively compute directory size in bytes.
mod disk;
pub(crate) use disk::{
    cap_simard_target_dirs, clean_simard_canaries, clean_stale_cargo_targets,
    remove_old_corrupt_dbs, rotate_simard_binary_backups, trim_simard_snapshots,
};

#[cfg(test)]
mod tests;
