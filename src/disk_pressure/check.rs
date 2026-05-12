//! `statvfs`-backed disk pressure check.
//!
//! Split from `mod.rs` so the production statvfs path is isolated from
//! the pure decision logic and the convenience wrappers.

use std::path::Path;

use super::{DiskPressureReport, PressureLevel};

/// Default minimum-free threshold (in GiB) when
/// `SIMARD_DISK_PRESSURE_MIN_FREE_GB` is unset.
pub const DEFAULT_MIN_FREE_GB: u64 = 20;

/// Snapshot of `statvfs` results used by the policy. Tests inject
/// synthetic values via [`DiskStatProvider`] so the band logic can be
/// exercised hermetically.
#[derive(Debug, Clone, Copy)]
pub struct DiskStat {
    pub free_bytes: u64,
    pub total_bytes: u64,
}

/// Indirection over the `statvfs` syscall so tests can substitute
/// arbitrary `(free, total)` pairs.
pub trait DiskStatProvider {
    fn stat(&self, path: &Path) -> Result<DiskStat, std::io::Error>;
}

/// Production provider — calls `nix::sys::statvfs::statvfs`.
pub struct RealDiskStatProvider;

impl DiskStatProvider for RealDiskStatProvider {
    fn stat(&self, path: &Path) -> Result<DiskStat, std::io::Error> {
        use nix::sys::statvfs::statvfs;
        let s = statvfs(path)
            .map_err(|e| std::io::Error::other(format!("statvfs({}): {e}", path.display())))?;
        // f_bavail = blocks available to unprivileged users (preferred
        // over f_bfree, which counts root-reserved blocks too).
        // f_frsize = fragment size; the real allocation unit on most
        // filesystems (and the one Linux's statvfs man page recommends).
        let free = (s.fragment_size() as u64).saturating_mul(s.blocks_available() as u64);
        let total = (s.fragment_size() as u64).saturating_mul(s.blocks() as u64);
        Ok(DiskStat {
            free_bytes: free,
            total_bytes: total,
        })
    }
}

/// Production entry point: stat `path` with the real provider and
/// classify the result against `min_free_gb`.
pub fn check_disk_pressure(
    path: &Path,
    min_free_gb: u64,
) -> Result<DiskPressureReport, std::io::Error> {
    check_disk_pressure_with(&RealDiskStatProvider, path, min_free_gb)
}

/// Provider-injectable form of [`check_disk_pressure`]. Pure once the
/// provider returns its `DiskStat`; tests use a fake provider to drive
/// the band classification.
pub fn check_disk_pressure_with<P: DiskStatProvider + ?Sized>(
    provider: &P,
    path: &Path,
    min_free_gb: u64,
) -> Result<DiskPressureReport, std::io::Error> {
    let stat = provider.stat(path)?;
    let threshold_bytes = min_free_gb.saturating_mul(1024 * 1024 * 1024);
    let level = PressureLevel::classify(stat.free_bytes, threshold_bytes);
    Ok(DiskPressureReport {
        path: path.to_path_buf(),
        free_bytes: stat.free_bytes,
        total_bytes: stat.total_bytes,
        threshold_bytes,
        level,
    })
}
