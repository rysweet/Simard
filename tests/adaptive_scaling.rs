//! TDD tests for PR3: AIMD Adaptive Scaling (issue #2182).
//!
//! Tests that `AdaptiveScaler`:
//! - Starts at the configured initial value
//! - Additively increases on low pressure
//! - Multiplicatively decreases after 429 errors
//! - Never exceeds ceiling or goes below floor
//! - Detects 429 from SimardError variants
//! - Ignores non-429 errors
//! - Clamps construction bounds correctly
//! - Integrates with decide() via OodaConfig
//!
//! Most tests will FAIL until the AIMD algorithm is implemented in
//! `adaptive_scaling.rs`. The stub provides correct construction but
//! no-op adjust()/report_error().

use simard::error::SimardError;
use simard::ooda_loop::adaptive_scaling::{
    AdaptiveScaler, DECREASE_FACTOR, HIGH_PRESSURE_THRESHOLD, LOW_PRESSURE_THRESHOLD,
};

fn adjust_without_system_pressure(s: &AdaptiveScaler) -> u32 {
    s.adjust_with_samples(None, None)
}

// ── Construction ──

#[test]
fn scaler_starts_at_initial_value() {
    let s = AdaptiveScaler::new(4, 1, 8);
    assert_eq!(s.current_max(), 4);
}

#[test]
fn scaler_clamps_zero_floor_to_one() {
    let s = AdaptiveScaler::new(5, 0, 10);
    assert_eq!(s.floor(), 1, "floor=0 should be raised to 1");
    assert!(s.current_max() >= 1);
}

#[test]
fn scaler_clamps_ceiling_below_floor() {
    let s = AdaptiveScaler::new(5, 4, 2);
    assert!(
        s.ceiling() >= s.floor(),
        "ceiling should be raised to at least floor"
    );
}

#[test]
fn scaler_clamps_initial_above_ceiling() {
    let s = AdaptiveScaler::new(100, 1, 8);
    assert_eq!(
        s.current_max(),
        8,
        "initial above ceiling should clamp to ceiling"
    );
}

#[test]
fn scaler_clamps_initial_below_floor() {
    let s = AdaptiveScaler::new(0, 2, 8);
    assert_eq!(
        s.current_max(),
        2,
        "initial below floor should clamp to floor"
    );
}

// ── AIMD behavior ──

#[test]
fn adjust_additive_increase_on_no_pressure() {
    let s = AdaptiveScaler::new(4, 1, 8);
    // With no pressure signals (no 429s, no /proc pressure), adjust
    // should increase by 1.
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
fn adjust_decrease_rounds_down_odd_values() {
    // 5 * 0.5 = 2.5 → should round to 2 (floor division).
    let s = AdaptiveScaler::new(5, 1, 8);
    let error = SimardError::AdapterInvocationFailed {
        base_type: "copilot-sdk".to_string(),
        reason: "HTTP 429 Too Many Requests".to_string(),
    };
    s.report_error(&error);
    let new_max = adjust_without_system_pressure(&s);
    assert_eq!(new_max, 2, "5 * 0.5 = 2.5 should round down to 2");
}

#[test]
fn adjust_decrease_from_3_goes_to_1() {
    // 3 * 0.5 = 1.5 → rounds to 1, which is >= floor(1).
    let s = AdaptiveScaler::new(3, 1, 8);
    let error = SimardError::AdapterInvocationFailed {
        base_type: "copilot-sdk".to_string(),
        reason: "HTTP 429 Too Many Requests".to_string(),
    };
    s.report_error(&error);
    let new_max = adjust_without_system_pressure(&s);
    assert!(
        new_max >= 1,
        "3 * 0.5 = 1.5 → should round to at least floor=1, got {new_max}"
    );
    assert!(
        new_max <= 2,
        "3 * 0.5 = 1.5 → should be 1 or 2, got {new_max}"
    );
}

// ── Bounds enforcement ──

#[test]
fn adjust_never_exceeds_ceiling() {
    let s = AdaptiveScaler::new(7, 1, 8);
    // Multiple no-pressure adjusts should cap at ceiling.
    for _ in 0..5 {
        let m = adjust_without_system_pressure(&s);
        assert!(m <= 8, "should never exceed ceiling of 8, got {m}");
    }
}

#[test]
fn adjust_reaches_ceiling_from_below() {
    let s = AdaptiveScaler::new(6, 1, 8);
    let m1 = adjust_without_system_pressure(&s); // 6 → 7
    assert_eq!(m1, 7, "should increase from 6 to 7");
    let m2 = adjust_without_system_pressure(&s); // 7 → 8
    assert_eq!(m2, 8, "should increase from 7 to 8");
    let m3 = adjust_without_system_pressure(&s); // 8 → 8 (capped)
    assert_eq!(m3, 8, "should stay at ceiling 8");
}

#[test]
fn adjust_never_goes_below_floor() {
    let s = AdaptiveScaler::new(2, 1, 8);
    let error = SimardError::AdapterInvocationFailed {
        base_type: "copilot-sdk".to_string(),
        reason: "HTTP 429 Too Many Requests".to_string(),
    };
    // Repeated 429s + adjusts should never go below floor.
    for _ in 0..10 {
        s.report_error(&error);
        let m = adjust_without_system_pressure(&s);
        assert!(m >= 1, "should never go below floor of 1, got {m}");
    }
}

#[test]
fn adjust_respects_custom_floor() {
    let s = AdaptiveScaler::new(4, 3, 8);
    let error = SimardError::AdapterInvocationFailed {
        base_type: "copilot-sdk".to_string(),
        reason: "HTTP 429 Too Many Requests".to_string(),
    };
    for _ in 0..10 {
        s.report_error(&error);
        let m = adjust_without_system_pressure(&s);
        assert!(m >= 3, "should never go below custom floor of 3, got {m}");
    }
}

// ── Error detection ──

#[test]
fn report_error_detects_429_in_adapter_invocation() {
    let s = AdaptiveScaler::new(4, 1, 8);

    // An AdapterInvocationFailed with "429" in the reason should trigger decrease.
    let error = SimardError::AdapterInvocationFailed {
        base_type: "copilot-sdk".to_string(),
        reason: "HTTP 429 Too Many Requests".to_string(),
    };
    s.report_error(&error);
    let m = adjust_without_system_pressure(&s);
    assert!(
        m < 4,
        "429 error should trigger decrease; expected < 4, got {m}"
    );
}

#[test]
fn report_error_detects_rate_limit_phrasing() {
    let s = AdaptiveScaler::new(4, 1, 8);

    // Alternative phrasing: "rate limit" without "429" literal.
    let error = SimardError::AdapterInvocationFailed {
        base_type: "copilot-sdk".to_string(),
        reason: "rate limit exceeded".to_string(),
    };
    s.report_error(&error);
    let m = adjust_without_system_pressure(&s);
    assert!(
        m < 4,
        "rate-limit error should trigger decrease; expected < 4, got {m}"
    );
}

#[test]
fn report_error_ignores_non_429_errors() {
    let s = AdaptiveScaler::new(4, 1, 8);

    // Non-429 errors should NOT trigger decrease.
    let error = SimardError::AdapterInvocationFailed {
        base_type: "copilot-sdk".to_string(),
        reason: "internal server error 500".to_string(),
    };
    s.report_error(&error);
    let m = adjust_without_system_pressure(&s);
    assert!(
        m >= 4,
        "non-429 errors should not trigger decrease; expected >= 4, got {m}"
    );
}

#[test]
fn report_error_ignores_unrelated_error_variants() {
    let s = AdaptiveScaler::new(4, 1, 8);

    // Completely different error variant.
    let error = SimardError::StoragePoisoned {
        store: "test".to_string(),
    };
    s.report_error(&error);
    let m = adjust_without_system_pressure(&s);
    assert!(
        m >= 4,
        "unrelated error variants should not trigger decrease; expected >= 4, got {m}"
    );
}

// ── Signal parsers (platform-gated) ──

#[cfg(target_os = "linux")]
#[test]
fn cpu_pressure_returns_value_on_linux() {
    use simard::ooda_loop::adaptive_scaling::sample_cpu_pressure;
    let pressure = sample_cpu_pressure();
    // On Linux, should eventually return Some after two samples.
    // First call may return None (no previous sample).
    // We just verify it doesn't panic.
    if let Some(p) = pressure {
        assert!(
            (0.0..=1.0).contains(&p),
            "CPU pressure should be in [0, 1], got {p}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn memory_pressure_returns_value_on_linux() {
    use simard::ooda_loop::adaptive_scaling::sample_memory_pressure;
    let pressure = sample_memory_pressure();
    // On Linux, should return Some(value) from /proc/meminfo.
    if let Some(p) = pressure {
        assert!(
            (0.0..=1.0).contains(&p),
            "memory pressure should be in [0, 1], got {p}"
        );
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn pressure_signals_return_none_on_non_linux() {
    use simard::ooda_loop::adaptive_scaling::{sample_cpu_pressure, sample_memory_pressure};
    assert_eq!(
        sample_cpu_pressure(),
        None,
        "CPU pressure should be None on non-Linux"
    );
    assert_eq!(
        sample_memory_pressure(),
        None,
        "memory pressure should be None on non-Linux"
    );
}

// ── Integration with OodaConfig / decide ──

#[test]
fn scaler_current_max_can_override_config() {
    use simard::ooda_loop::{OodaConfig, Priority, decide};

    let scaler = AdaptiveScaler::new(2, 1, 8);

    // Create priorities that would normally produce more than 2 actions.
    let priorities: Vec<Priority> = (1..=5)
        .map(|i| Priority {
            goal_id: format!("g{i}"),
            urgency: 1.0 - (i as f64 * 0.1),
            reason: format!("priority {i}"),
        })
        .collect();

    // Use scaler's current_max as the config limit.
    let config = OodaConfig {
        max_concurrent_actions: scaler.current_max(),
        // Pin the scaler off so an ambient `SIMARD_SCALING=auto` (set by a
        // concurrent test) cannot leak a default 24-wide scaler in via
        // `default()` and defeat the explicit cap. That non-hermeticity is what
        // flaked this test RED and turned main RED after PR #4361 (#4361).
        scaler: None,
        ..OodaConfig::default()
    };

    let actions = decide(&priorities, &config).unwrap();
    assert!(
        actions.len() <= 2,
        "decide should respect scaler's current_max of 2; got {} actions",
        actions.len()
    );
}

// ── Constants validation ──

// ── Hermeticity regression (P1 / RED main, issue #4361 follow-up) ──
//
// `scaler_current_max_can_override_config` (above) used to build its config
// with a bare `..OodaConfig::default()`, and `OodaConfig::default()` reads
// `SIMARD_SCALING` from the *process* environment. When any concurrent test in
// the run set `SIMARD_SCALING=auto`, `default()` populated the `scaler` field,
// and `decide` prefers the scaler's adjusted limit over the explicit
// `max_concurrent_actions`. The override cap of 2 was silently defeated and the
// assertion flaked RED — the scheduling-dependent break that turned main RED
// after PR #4361.
//
// This test locks in the fix *deterministically*: it sets `SIMARD_SCALING=auto`
// for the duration of the config build (serialised via the `cognitive_memory`
// key so no other env writer can interleave) and then exercises the override
// path. The construction pins `scaler: None`, so an ambient `SIMARD_SCALING`
// cannot leak a 24-wide scaler in; the cap holds at 2. If a future change drops
// the `scaler: None` pin and reverts to a bare `..default()`, this guard flips
// RED again under the injected `SIMARD_SCALING=auto`.
#[test]
#[serial_test::serial(cognitive_memory)]
fn scaler_override_config_is_hermetic_against_ambient_scaling() {
    use simard::ooda_loop::{OodaConfig, Priority, decide};

    let scaler = AdaptiveScaler::new(2, 1, 8);
    let priorities: Vec<Priority> = (1..=5)
        .map(|i| Priority {
            goal_id: format!("g{i}"),
            urgency: 1.0 - (i as f64 * 0.1),
            reason: format!("priority {i}"),
        })
        .collect();

    let prev = std::env::var_os("SIMARD_SCALING");
    // SAFETY: serialised via #[serial(cognitive_memory)]; no concurrent env
    // mutation can tear this write.
    unsafe {
        std::env::set_var("SIMARD_SCALING", "auto");
    }

    let result = std::panic::catch_unwind(|| {
        // The override path MUST NOT inherit an ambient scaler. Pinning
        // `scaler: None` keeps the explicit `max_concurrent_actions` cap in
        // force even though `SIMARD_SCALING=auto` is set in the environment; a
        // bare `..OodaConfig::default()` here would leak that ambient scaler and
        // break the cap.
        let config = OodaConfig {
            max_concurrent_actions: scaler.current_max(),
            scaler: None,
            ..OodaConfig::default()
        };
        let actions = decide(&priorities, &config).unwrap();
        assert!(
            actions.len() <= 2,
            "override config must cap at scaler.current_max()=2 regardless of ambient \
             SIMARD_SCALING; got {} actions (ambient scaler leaked in)",
            actions.len()
        );
    });

    // SAFETY: restore before propagating any panic (same serial key).
    unsafe {
        match prev {
            Some(v) => std::env::set_var("SIMARD_SCALING", v),
            None => std::env::remove_var("SIMARD_SCALING"),
        }
    }
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

#[test]
fn aimd_constants_are_sensible() {
    // Validate relationship between thresholds (const assertions).
    const {
        assert!(HIGH_PRESSURE_THRESHOLD > LOW_PRESSURE_THRESHOLD);
    }
    assert!(
        (0.0..=1.0).contains(&HIGH_PRESSURE_THRESHOLD),
        "high threshold should be in [0, 1]"
    );
    assert!(
        (0.0..=1.0).contains(&LOW_PRESSURE_THRESHOLD),
        "low threshold should be in [0, 1]"
    );
    assert!(
        (0.0..1.0).contains(&DECREASE_FACTOR),
        "decrease factor should be in (0, 1)"
    );
}

// ── Thread safety ──

#[test]
fn scaler_is_safe_to_share_across_threads() {
    use std::sync::Arc;
    use std::thread;

    let scaler = Arc::new(AdaptiveScaler::new(4, 1, 8));
    let mut handles = vec![];

    // Concurrent reads.
    for _ in 0..4 {
        let s = Arc::clone(&scaler);
        handles.push(thread::spawn(move || {
            let m = s.current_max();
            assert!((1..=8).contains(&m), "should be in bounds, got {m}");
        }));
    }

    // Concurrent adjusts.
    for _ in 0..4 {
        let s = Arc::clone(&scaler);
        handles.push(thread::spawn(move || {
            let m = adjust_without_system_pressure(&s);
            assert!((1..=8).contains(&m), "should be in bounds, got {m}");
        }));
    }

    for h in handles {
        h.join().expect("thread should not panic");
    }
}
