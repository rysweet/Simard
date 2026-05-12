//! Disk-pressure precheck (issue #1697 follow-up).
//!
//! Refuses to allocate a new engineer worktree when the filesystem
//! hosting the worktrees root has less than the configured amount of
//! free space. Default threshold: **20 GiB**.
//!
//! # Decision logic
//!
//! Given a configured `min_free_gb` (call the byte-equivalent `T`):
//!
//!   - `free >= T`            → [`PressureLevel::Ok`]     — proceed.
//!   - `T/2 <= free < T`      → [`PressureLevel::Warn`]   — proceed but log loud.
//!   - `free < T/2`           → [`PressureLevel::Refuse`] — caller MUST abort.
//!
//! The two-band design ("warn at 50-100% of threshold; refuse below 50%")
//! gives an operator one OODA cycle of warning before the refuse line
//! triggers, instead of slamming straight from green to red.
//!
//! # Why this exists
//!
//! The disk-fill incident (issue #1697) ENOSPC-killed cognitive-memory
//! WAL writes mid-cycle and corrupted some engineer subprocesses. The
//! cargo-target-dir fix (PR A) and worktree GC (PR B) together prevent
//! the leak; this precheck is the **belt** complementing those two
//! braces — it ensures that even if a future leak slips past the GC,
//! we refuse new work before the daemon writes itself to ENOSPC again.

use std::path::{Path, PathBuf};

pub mod check;

#[cfg(test)]
mod tests;

pub use check::{
    DEFAULT_MIN_FREE_GB, DiskStat, DiskStatProvider, RealDiskStatProvider, check_disk_pressure,
    check_disk_pressure_with,
};

/// Result of a single `check_disk_pressure` call.
#[derive(Debug, Clone)]
pub struct DiskPressureReport {
    /// Path that was checked.
    pub path: PathBuf,
    /// Free bytes reported by `statvfs`.
    pub free_bytes: u64,
    /// Total bytes reported by `statvfs` (filesystem capacity).
    pub total_bytes: u64,
    /// Threshold used for this evaluation, in bytes.
    pub threshold_bytes: u64,
    /// Decision level. See module-level docs for the bands.
    pub level: PressureLevel,
}

impl DiskPressureReport {
    /// `true` iff the level forbids new allocations.
    pub fn should_refuse(&self) -> bool {
        self.level == PressureLevel::Refuse
    }

    /// Compose the operator-facing reason string used by the worktree
    /// allocator's failure path. Includes free space, threshold, the
    /// path checked, and the suggested remediation.
    pub fn refuse_message(&self) -> String {
        format!(
            "disk pressure REFUSED: only {} free at {} (threshold {}); \
             run `simard worktree-gc --apply` to reclaim space, then retry",
            human_bytes(self.free_bytes),
            self.path.display(),
            human_bytes(self.threshold_bytes),
        )
    }

    /// Compose a one-line warn message for log-only emission.
    pub fn warn_message(&self) -> String {
        format!(
            "disk pressure WARN: {} free at {} (threshold {}); \
             approaching refuse line, consider `simard worktree-gc --apply`",
            human_bytes(self.free_bytes),
            self.path.display(),
            human_bytes(self.threshold_bytes),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureLevel {
    /// `free >= threshold` — proceed normally.
    Ok,
    /// `threshold/2 <= free < threshold` — proceed but emit a warning.
    Warn,
    /// `free < threshold/2` — caller MUST abort.
    Refuse,
}

impl PressureLevel {
    /// Pure decision function. Lifted out so the policy can be unit-
    /// tested without touching `statvfs`.
    pub fn classify(free_bytes: u64, threshold_bytes: u64) -> Self {
        let half = threshold_bytes / 2;
        if free_bytes >= threshold_bytes {
            PressureLevel::Ok
        } else if free_bytes >= half {
            PressureLevel::Warn
        } else {
            PressureLevel::Refuse
        }
    }
}

/// Helper: render a byte count in IEC units (GiB / MiB / KiB) at one
/// decimal place. Always shows a trailing unit so log lines align well
/// when grep'd.
pub fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Resolve the configured minimum-free threshold (in GiB) from
/// `SIMARD_DISK_PRESSURE_MIN_FREE_GB`, falling back to
/// [`DEFAULT_MIN_FREE_GB`]. Negative or unparseable values fall back to
/// the default with a WARN log so the operator notices the typo.
pub fn configured_min_free_gb() -> u64 {
    match std::env::var("SIMARD_DISK_PRESSURE_MIN_FREE_GB") {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(n) if n > 0 => n,
            Ok(_) => {
                tracing::warn!(
                    target: "simard::disk_pressure",
                    raw = %raw,
                    "SIMARD_DISK_PRESSURE_MIN_FREE_GB=0 is invalid; using default {}",
                    DEFAULT_MIN_FREE_GB,
                );
                DEFAULT_MIN_FREE_GB
            }
            Err(e) => {
                tracing::warn!(
                    target: "simard::disk_pressure",
                    raw = %raw,
                    error = %e,
                    "SIMARD_DISK_PRESSURE_MIN_FREE_GB unparseable; using default {}",
                    DEFAULT_MIN_FREE_GB,
                );
                DEFAULT_MIN_FREE_GB
            }
        },
        Err(_) => DEFAULT_MIN_FREE_GB,
    }
}

/// Convenience wrapper used from the engineer-worktree allocator: do a
/// single check at `path` against the env-configured threshold and emit
/// the appropriate log line for `Warn`.
///
/// Returns the report. The caller is responsible for honoring
/// [`DiskPressureReport::should_refuse`] — a `Refuse` from this function
/// must abort the surrounding action.
pub fn check_with_default_threshold(path: &Path) -> Result<DiskPressureReport, std::io::Error> {
    let min_free_gb = configured_min_free_gb();
    let report = check_disk_pressure(path, min_free_gb)?;
    match report.level {
        PressureLevel::Ok => {}
        PressureLevel::Warn => {
            tracing::warn!(
                target: "simard::disk_pressure",
                free_bytes = report.free_bytes,
                threshold_bytes = report.threshold_bytes,
                path = %report.path.display(),
                "{}",
                report.warn_message(),
            );
        }
        PressureLevel::Refuse => {
            tracing::error!(
                target: "simard::disk_pressure",
                free_bytes = report.free_bytes,
                threshold_bytes = report.threshold_bytes,
                path = %report.path.display(),
                "{}",
                report.refuse_message(),
            );
        }
    }
    Ok(report)
}
