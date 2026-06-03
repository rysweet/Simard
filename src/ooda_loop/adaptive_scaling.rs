//! AIMD adaptive scaling for `max_concurrent_actions`.
//!
//! Dynamically adjusts OODA cycle concurrency based on system pressure
//! signals from `/proc/stat` (CPU), `/proc/meminfo` (memory), and
//! Copilot 429 error responses.
//!
//! Controlled by `SIMARD_SCALING` env var: `auto` enables AIMD, `fixed`
//! (or unset) disables it. See `docs/reference/adaptive-scaling-api.md`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::SimardError;

/// Pressure above this triggers multiplicative decrease.
pub const HIGH_PRESSURE_THRESHOLD: f64 = 0.8;
/// Pressure below this triggers additive increase.
pub const LOW_PRESSURE_THRESHOLD: f64 = 0.3;
/// Multiplicative decrease factor (halve on pressure).
pub const DECREASE_FACTOR: f64 = 0.5;
/// Sliding window in seconds for counting 429 errors.
pub const ERROR_WINDOW_SECS: u64 = 300;

/// AIMD adaptive scaler for `max_concurrent_actions`.
///
/// Uses `AtomicU32` with `Relaxed` ordering because the concurrency
/// limit is an independent numeric throttle — no other shared state
/// depends on its memory visibility.
pub struct AdaptiveScaler {
    current: AtomicU32,
    floor: u32,
    ceiling: u32,
    /// Timestamps (unix epoch secs) of recent 429/rate-limit errors.
    error_timestamps: Mutex<Vec<u64>>,
}

impl std::fmt::Debug for AdaptiveScaler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdaptiveScaler")
            .field("current", &self.current.load(Ordering::Relaxed))
            .field("floor", &self.floor)
            .field("ceiling", &self.ceiling)
            .finish()
    }
}

impl AdaptiveScaler {
    /// Creates a new scaler with clamped bounds:
    /// - `floor` is raised to at least 1 (zero would disable dispatch).
    /// - `ceiling` is raised to at least `floor`.
    /// - `initial` is clamped to `[floor, ceiling]`.
    pub fn new(initial: u32, floor: u32, ceiling: u32) -> Self {
        let floor = floor.max(1);
        let ceiling = ceiling.max(floor);
        let initial = initial.clamp(floor, ceiling);
        Self {
            current: AtomicU32::new(initial),
            floor,
            ceiling,
            error_timestamps: Mutex::new(Vec::with_capacity(16)),
        }
    }

    /// Returns the current `max_concurrent_actions` value.
    pub fn current_max(&self) -> u32 {
        self.current.load(Ordering::Relaxed)
    }

    /// Returns the configured floor.
    pub fn floor(&self) -> u32 {
        self.floor
    }

    /// Returns the configured ceiling.
    pub fn ceiling(&self) -> u32 {
        self.ceiling
    }

    /// Samples system pressure and adjusts the concurrency limit.
    /// Called once per OODA cycle, before the Decide phase.
    ///
    /// Returns the new max value after adjustment.
    pub fn adjust(&self) -> u32 {
        self.adjust_with_samples(sample_cpu_pressure(), sample_memory_pressure())
    }

    #[doc(hidden)]
    pub fn adjust_with_samples(&self, cpu_pressure: Option<f64>, mem_pressure: Option<f64>) -> u32 {
        let now_epoch = epoch_secs();
        let current = self.current.load(Ordering::Relaxed);

        // Count recent 429 errors in the sliding window.
        let has_recent_429 = {
            let mut timestamps = self
                .error_timestamps
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let cutoff = now_epoch.saturating_sub(ERROR_WINDOW_SECS);
            timestamps.retain(|&t| t >= cutoff);
            !timestamps.is_empty()
        };

        let system_pressure = cpu_pressure.unwrap_or(0.0).max(mem_pressure.unwrap_or(0.0));

        // AIMD rule:
        // - 429 errors or high system pressure → multiplicative decrease
        // - low pressure and no 429s → additive increase
        // - moderate → hold steady
        let new = if has_recent_429 || system_pressure > HIGH_PRESSURE_THRESHOLD {
            let decreased = (current as f64 * DECREASE_FACTOR) as u32;
            decreased.max(self.floor)
        } else if system_pressure < LOW_PRESSURE_THRESHOLD {
            (current + 1).min(self.ceiling)
        } else {
            current
        };

        self.current.store(new, Ordering::Relaxed);
        new
    }

    /// Reports an action error. When the error carries an HTTP 429
    /// status or rate-limit indication, records a pressure signal for
    /// the next `adjust()` call.
    pub fn report_error(&self, error: &SimardError) {
        if let SimardError::AdapterInvocationFailed { reason, .. } = error {
            self.report_reason(reason);
        }
    }

    /// Records a pressure signal when `reason` contains "429" or "rate limit"
    /// (case-insensitive). Avoids constructing a [`SimardError`] when the
    /// caller already has the reason string (e.g. from `ActionOutcome.detail`).
    pub fn report_reason(&self, reason: &str) {
        if contains_ascii_case_insensitive(reason, b"429")
            || contains_ascii_case_insensitive(reason, b"rate limit")
        {
            let now = epoch_secs();
            if let Ok(mut timestamps) = self.error_timestamps.lock() {
                timestamps.push(now);
            }
        }
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Case-insensitive ASCII substring search without allocation.
fn contains_ascii_case_insensitive(haystack: &str, needle: &[u8]) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

/// Returns CPU pressure as `[0.0, 1.0]`, or `None` on non-Linux / parse failure.
///
/// Reads `/proc/stat` and computes `1.0 - idle_ratio` from the cumulative
/// CPU counters (user, nice, system, idle, iowait, irq, softirq, steal).
#[cfg(target_os = "linux")]
pub fn sample_cpu_pressure() -> Option<f64> {
    let content = std::fs::read_to_string("/proc/stat").ok()?;
    let cpu_line = content.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = cpu_line
        .split_whitespace()
        .skip(1) // skip "cpu" label
        .filter_map(|f| f.parse().ok())
        .collect();
    if fields.len() < 4 {
        return None;
    }
    let total: u64 = fields.iter().sum();
    if total == 0 {
        return None;
    }
    // idle is the 4th field (index 3); iowait is the 5th (index 4) if present
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some(1.0 - (idle as f64 / total as f64))
}

/// Fallback: always `None` on non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn sample_cpu_pressure() -> Option<f64> {
    None
}

/// Returns memory pressure as `[0.0, 1.0]`, or `None` on non-Linux / parse failure.
///
/// Reads `/proc/meminfo` and computes `1.0 - MemAvailable / MemTotal`.
#[cfg(target_os = "linux")]
pub fn sample_memory_pressure() -> Option<f64> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total: Option<u64> = None;
    let mut available: Option<u64> = None;
    for line in content.lines() {
        if line.starts_with("MemTotal:") {
            total = parse_meminfo_kb(line);
        } else if line.starts_with("MemAvailable:") {
            available = parse_meminfo_kb(line);
        }
        if total.is_some() && available.is_some() {
            break;
        }
    }
    let total = total.filter(|&t| t > 0)?;
    let available = available?;
    Some(1.0 - (available as f64 / total as f64))
}

/// Parse a `/proc/meminfo` line like `MemTotal:       16384 kB` into kB value.
#[cfg(target_os = "linux")]
fn parse_meminfo_kb(line: &str) -> Option<u64> {
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Fallback: always `None` on non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn sample_memory_pressure() -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adjust_without_system_pressure(s: &AdaptiveScaler) -> u32 {
        s.adjust_with_samples(None, None)
    }

    #[test]
    fn construction_clamps_zero_floor_to_one() {
        let s = AdaptiveScaler::new(5, 0, 10);
        assert_eq!(s.floor(), 1);
    }

    #[test]
    fn construction_clamps_ceiling_below_floor() {
        let s = AdaptiveScaler::new(5, 4, 2);
        // ceiling should be raised to at least floor
        assert!(s.ceiling() >= s.floor());
    }

    #[test]
    fn construction_clamps_initial_to_range() {
        let s = AdaptiveScaler::new(100, 1, 8);
        assert_eq!(s.current_max(), 8);

        let s2 = AdaptiveScaler::new(0, 2, 8);
        assert_eq!(s2.current_max(), 2);
    }

    #[test]
    fn cpu_pressure_returns_option() {
        // On any platform, should not panic.
        let _p = sample_cpu_pressure();
    }

    #[test]
    fn memory_pressure_returns_option() {
        let _p = sample_memory_pressure();
    }

    #[test]
    fn adjust_additive_increase_on_no_pressure() {
        let s = AdaptiveScaler::new(4, 1, 8);
        // With no pressure signals, adjust should increase by 1.
        let new_max = adjust_without_system_pressure(&s);
        assert_eq!(
            new_max, 5,
            "adjust() with no pressure should additive-increase from 4 to 5"
        );
    }

    #[test]
    fn adjust_multiplicative_decrease_after_429() {
        let s = AdaptiveScaler::new(4, 1, 8);
        let error = SimardError::AdapterInvocationFailed {
            base_type: "copilot-sdk".to_string(),
            reason: "HTTP 429 Too Many Requests".to_string(),
        };
        s.report_error(&error);
        let new_max = adjust_without_system_pressure(&s);
        assert_eq!(new_max, 2, "adjust() after 429 should halve from 4 to 2");
    }

    #[test]
    fn report_error_ignores_non_429() {
        let s = AdaptiveScaler::new(4, 1, 8);
        let error = SimardError::AdapterInvocationFailed {
            base_type: "copilot-sdk".to_string(),
            reason: "internal server error".to_string(),
        };
        s.report_error(&error);
        // Non-429 errors should not affect the scaler.
        // adjust() without pressure should still increase.
        let new_max = adjust_without_system_pressure(&s);
        assert_eq!(
            new_max, 5,
            "non-429 errors should not trigger decrease; expected increase to 5"
        );
    }

    #[test]
    fn adjust_never_exceeds_ceiling() {
        let s = AdaptiveScaler::new(7, 1, 8);
        let m1 = adjust_without_system_pressure(&s);
        assert!(m1 <= 8, "should not exceed ceiling of 8, got {m1}");
        let m2 = adjust_without_system_pressure(&s);
        assert!(m2 <= 8, "should not exceed ceiling of 8, got {m2}");
    }

    #[test]
    fn adjust_never_goes_below_floor() {
        let s = AdaptiveScaler::new(2, 1, 8);
        let error = SimardError::AdapterInvocationFailed {
            base_type: "copilot-sdk".to_string(),
            reason: "HTTP 429 Too Many Requests".to_string(),
        };
        // Report many 429s and adjust — should never go below 1.
        for _ in 0..10 {
            s.report_error(&error);
            let m = adjust_without_system_pressure(&s);
            assert!(m >= 1, "should never go below floor of 1, got {m}");
        }
    }

    #[test]
    fn debug_impl_shows_current_floor_ceiling() {
        let s = AdaptiveScaler::new(4, 1, 8);
        let debug = format!("{s:?}");
        assert!(debug.contains("AdaptiveScaler"), "Debug output: {debug}");
        assert!(debug.contains("current: 4"), "Debug output: {debug}");
        assert!(debug.contains("floor: 1"), "Debug output: {debug}");
        assert!(debug.contains("ceiling: 8"), "Debug output: {debug}");
    }

    // -----------------------------------------------------------------------
    // Issue #2182: additional scaler wiring tests
    // -----------------------------------------------------------------------

    #[test]
    fn report_error_tracks_rate_limit_phrase() {
        let s = AdaptiveScaler::new(4, 1, 8);
        let error = SimardError::AdapterInvocationFailed {
            base_type: "copilot-sdk".to_string(),
            reason: "rate limit exceeded — slow down".to_string(),
        };
        s.report_error(&error);
        let new_max = adjust_without_system_pressure(&s);
        assert_eq!(
            new_max, 2,
            "\"rate limit\" phrase should trigger decrease: 4 → 2"
        );
    }

    #[test]
    fn consecutive_429s_reduce_to_floor() {
        let s = AdaptiveScaler::new(8, 1, 16);
        let error = SimardError::AdapterInvocationFailed {
            base_type: "copilot-sdk".to_string(),
            reason: "HTTP 429 Too Many Requests".to_string(),
        };
        // Each adjust-after-429 halves: 8 → 4 → 2 → 1
        for _ in 0..5 {
            s.report_error(&error);
            adjust_without_system_pressure(&s);
        }
        let final_val = s.current_max();
        assert_eq!(
            final_val, 1,
            "repeated 429s must drive current_max to floor=1, got {final_val}"
        );
    }

    #[test]
    fn non_429_non_rate_limit_errors_ignored_by_report() {
        let s = AdaptiveScaler::new(4, 1, 8);
        let error = SimardError::AdapterInvocationFailed {
            base_type: "copilot-sdk".to_string(),
            reason: "connection timeout after 30s".to_string(),
        };
        s.report_error(&error);
        // Without 429 or rate-limit, adjust should increase
        let new_max = adjust_without_system_pressure(&s);
        assert_eq!(
            new_max, 5,
            "non-rate-limit errors must not trigger decrease, expected 5"
        );
    }

    #[test]
    fn report_error_only_matches_adapter_invocation_failed_variant() {
        // Verify that other SimardError variants are silently ignored
        let s = AdaptiveScaler::new(4, 1, 8);
        // PersistentStoreIo is a different variant — should be a no-op
        let error = SimardError::PersistentStoreIo {
            store: "test".to_string(),
            action: "read".to_string(),
            path: std::path::PathBuf::from("/tmp/fake"),
            reason: "429 in store path".to_string(),
        };
        s.report_error(&error);
        let new_max = adjust_without_system_pressure(&s);
        assert_eq!(
            new_max, 5,
            "non-AdapterInvocationFailed errors must not affect scaler"
        );
    }
}
