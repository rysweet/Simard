//! Per-cycle RSS health monitoring for the OODA daemon (issue #2167).
//!
//! Reads `/proc/self/statm` on Linux to report the daemon's resident set
//! size in bytes. Configurable warn and hard thresholds trigger log output
//! so operators are alerted before the OOM killer intervenes.

use std::fs;

/// RSS health check result.
#[derive(Clone, Debug)]
pub struct RssReport {
    /// Resident set size in bytes.
    pub rss_bytes: u64,
    /// True when RSS exceeds the warn threshold.
    pub warn: bool,
    /// True when RSS exceeds the hard threshold.
    pub critical: bool,
}

/// Default warn threshold: 4 GiB.
const DEFAULT_WARN_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Default hard threshold: 16 GiB.
const DEFAULT_HARD_BYTES: u64 = 16 * 1024 * 1024 * 1024;

/// Read the current process's RSS from `/proc/self/statm`.
///
/// Returns `None` on non-Linux platforms or if the file cannot be read.
pub fn read_rss_bytes() -> Option<u64> {
    let content = fs::read_to_string("/proc/self/statm").ok()?;
    // statm fields: size resident shared text lib data dt (all in pages)
    let resident_pages: u64 = content.split_whitespace().nth(1)?.parse().ok()?;
    let page_size = page_size();
    Some(resident_pages * page_size)
}

/// Check RSS against configurable thresholds and log warnings.
///
/// Thresholds are read from environment variables:
/// - `SIMARD_RSS_WARN_BYTES` (default: 4 GiB)
/// - `SIMARD_RSS_HARD_BYTES` (default: 16 GiB)
pub fn check_rss_health() -> Option<RssReport> {
    let rss_bytes = read_rss_bytes()?;
    let warn_threshold = env_u64("SIMARD_RSS_WARN_BYTES", DEFAULT_WARN_BYTES);
    let hard_threshold = env_u64("SIMARD_RSS_HARD_BYTES", DEFAULT_HARD_BYTES);

    let warn = rss_bytes >= warn_threshold;
    let critical = rss_bytes >= hard_threshold;

    let report = RssReport {
        rss_bytes,
        warn,
        critical,
    };

    if critical {
        tracing::error!(
            target: "simard::rss_health",
            rss_mb = rss_bytes / (1024 * 1024),
            threshold_mb = hard_threshold / (1024 * 1024),
            "CRITICAL: RSS exceeds hard threshold",
        );
    } else if warn {
        tracing::warn!(
            target: "simard::rss_health",
            rss_mb = rss_bytes / (1024 * 1024),
            threshold_mb = warn_threshold / (1024 * 1024),
            "RSS exceeds warn threshold",
        );
    }

    Some(report)
}

/// Format RSS in human-readable form for log output.
pub fn format_rss(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!("{} MiB", bytes / (1024 * 1024))
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn page_size() -> u64 {
    // sysconf(_SC_PAGESIZE) — safe, infallible on Linux.
    #[cfg(unix)]
    {
        // SAFETY: sysconf(_SC_PAGESIZE) is always valid on Linux/macOS.
        let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if ps > 0 { ps as u64 } else { 4096 }
    }
    #[cfg(not(unix))]
    {
        4096
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_rss_mib() {
        assert_eq!(format_rss(512 * 1024 * 1024), "512 MiB");
    }

    #[test]
    fn format_rss_gib() {
        assert_eq!(format_rss(4 * 1024 * 1024 * 1024), "4.0 GiB");
    }

    #[test]
    fn read_rss_returns_some_on_linux() {
        // On Linux CI this should succeed; on other platforms it returns None.
        if cfg!(target_os = "linux") {
            let rss = read_rss_bytes();
            assert!(rss.is_some(), "should read RSS on Linux");
            assert!(rss.unwrap() > 0, "RSS must be positive");
        }
    }

    #[test]
    fn check_rss_health_does_not_panic() {
        // Just verify it doesn't crash regardless of platform.
        let _ = check_rss_health();
    }
}
