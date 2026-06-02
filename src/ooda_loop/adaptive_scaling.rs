//! AIMD adaptive scaling for `max_concurrent_actions`.
//!
//! Dynamically adjusts OODA cycle concurrency based on system pressure
//! signals from `/proc/stat` (CPU), `/proc/meminfo` (memory), and
//! Copilot 429 error responses.
//!
//! Controlled by `SIMARD_SCALING` env var: `auto` enables AIMD, `fixed`
//! (or unset) disables it. See `docs/reference/adaptive-scaling-api.md`.

use std::sync::atomic::{AtomicU32, Ordering};

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
        // TODO(#2182): Implement AIMD algorithm:
        // 1. Sample CPU pressure from /proc/stat
        // 2. Sample memory pressure from /proc/meminfo
        // 3. Count 429 errors in sliding window
        // 4. Compute aggregate pressure (max of signals)
        // 5. Apply AIMD rule:
        //    - high pressure or 429s → multiplicative decrease
        //    - low pressure → additive increase
        //    - moderate → hold steady
        self.current.load(Ordering::Relaxed)
    }

    /// Reports an action error. When the error carries an HTTP 429
    /// status (via `AdapterInvocationFailed` with "429" in the reason),
    /// records a pressure signal for the next `adjust()` call.
    pub fn report_error(&self, _error: &SimardError) {
        // TODO(#2182): Detect 429 errors and record timestamp
        // in sliding window. Pattern-match on:
        //   SimardError::AdapterInvocationFailed { reason, .. }
        //   where reason contains "429"
    }
}

/// Returns CPU pressure as `[0.0, 1.0]`, or `None` on non-Linux / parse failure.
#[cfg(target_os = "linux")]
pub fn sample_cpu_pressure() -> Option<f64> {
    // TODO(#2182): Read /proc/stat, compute 1 - idle_ratio
    None
}

/// Fallback: always `None` on non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn sample_cpu_pressure() -> Option<f64> {
    None
}

/// Returns memory pressure as `[0.0, 1.0]`, or `None` on non-Linux / parse failure.
#[cfg(target_os = "linux")]
pub fn sample_memory_pressure() -> Option<f64> {
    // TODO(#2182): Read /proc/meminfo, compute 1 - available/total
    None
}

/// Fallback: always `None` on non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn sample_memory_pressure() -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let new_max = s.adjust();
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
        let new_max = s.adjust();
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
        let new_max = s.adjust();
        assert_eq!(
            new_max, 5,
            "non-429 errors should not trigger decrease; expected increase to 5"
        );
    }

    #[test]
    fn adjust_never_exceeds_ceiling() {
        let s = AdaptiveScaler::new(7, 1, 8);
        let m1 = s.adjust();
        assert!(m1 <= 8, "should not exceed ceiling of 8, got {m1}");
        let m2 = s.adjust();
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
            let m = s.adjust();
            assert!(m >= 1, "should never go below floor of 1, got {m}");
        }
    }
}
