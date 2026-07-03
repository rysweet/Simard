//! M1 configuration: the `SIMARD_OVERSEER_*` flag-gate (default **OFF**) and the
//! single-sourced daily-budget knob.
//!
//! Everything here is pure and env-**injectable** (`impl Fn(&str) -> Option<String>`),
//! so the gating and budget-resolution logic is unit-tested with zero process
//! environment mutation (no global state, no `set_var` races between parallel
//! tests). The `*_env` production entry points are the only functions that read
//! the real `std::env`.
//!
//! Two operator hard-gates live here:
//! - The Overseer is **additive and default-off**: absent/empty/unrecognised
//!   `SIMARD_OVERSEER_ENABLED` keeps the daemon behaving exactly as before.
//! - The daily budget is **single-sourced** from `SIMARD_DAILY_BUDGET_USD`
//!   (crusty risk #6) so the Overseer's [`BudgetGate`](crate::overseer::BudgetGate)
//!   can never drift from the OODA loop's ceiling.

/// Master flag that gates the Overseer on. Default **OFF**: while unset (or set
/// to a non-truthy value) nothing constructs or schedules an `Overseer` and the
/// daemon's behaviour is unchanged.
pub const OVERSEER_ENABLED_ENV: &str = "SIMARD_OVERSEER_ENABLED";

/// Daily LLM-budget ceiling, shared with the OODA loop. Single-sourcing this
/// (rather than hardcoding a duplicate) keeps the Overseer's budget gate and the
/// daemon's spend ceiling from silently diverging.
pub const DAILY_BUDGET_ENV: &str = "SIMARD_DAILY_BUDGET_USD";

/// Cadence (seconds) of the M1 read-only observer sensor. Clamped to a floor so
/// self-tuning (M4) can never drive the observer into a hot loop.
pub const OVERSEER_INTERVAL_ENV: &str = "SIMARD_OVERSEER_INTERVAL_SECS";

/// Default observer cadence: 15 minutes — frequent enough to catch churn, far
/// below any launch/merge cadence (M1 files at most one deduped issue per
/// recurring signature, so a tighter interval adds no writes).
pub const DEFAULT_OVERSEER_INTERVAL_SECS: u64 = 900;

/// Hard floor on the observer cadence (crusty risk #4 / M4 clamp): never tick
/// more often than once a minute regardless of env or self-tuning.
pub const MIN_OVERSEER_INTERVAL_SECS: u64 = 60;

/// Ultimate fallback when `SIMARD_DAILY_BUDGET_USD` is unset, empty, unparseable,
/// or non-positive. Mirrors the OODA loop's historical default.
pub const DEFAULT_DAILY_BUDGET_USD: f64 = 500.0;

/// Resolve the Overseer master flag from an env resolver. Fail-safe: only an
/// explicit truthy value (`1`/`true`/`yes`/`on`, case-insensitive) enables the
/// Overseer; everything else — including an unset var — leaves it OFF.
pub fn overseer_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool {
    match lookup(OVERSEER_ENABLED_ENV) {
        Some(v) => is_truthy(&v),
        None => false,
    }
}

/// Production entry point: read the real process environment.
pub fn overseer_enabled() -> bool {
    overseer_enabled_from(|k| std::env::var(k).ok())
}

/// Resolve the daily budget from an env resolver, falling back to
/// [`DEFAULT_DAILY_BUDGET_USD`] when the value is unset, empty, unparseable, or
/// non-positive. This is the single source of truth the [`BudgetGate`] reads.
///
/// [`BudgetGate`]: crate::overseer::BudgetGate
pub fn resolve_daily_budget_usd(lookup: impl Fn(&str) -> Option<String>) -> f64 {
    match lookup(DAILY_BUDGET_ENV).as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => match s.parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 => v,
            _ => DEFAULT_DAILY_BUDGET_USD,
        },
        _ => DEFAULT_DAILY_BUDGET_USD,
    }
}

/// Production entry point: read the real process environment.
pub fn daily_budget_usd() -> f64 {
    resolve_daily_budget_usd(|k| std::env::var(k).ok())
}

/// Resolve the observer cadence from an env resolver, clamped to
/// [`MIN_OVERSEER_INTERVAL_SECS`]. Unset/empty/unparseable → the default.
pub fn resolve_interval_secs(lookup: impl Fn(&str) -> Option<String>) -> u64 {
    let requested = match lookup(OVERSEER_INTERVAL_ENV).as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s.parse::<u64>().unwrap_or(DEFAULT_OVERSEER_INTERVAL_SECS),
        _ => DEFAULT_OVERSEER_INTERVAL_SECS,
    };
    requested.max(MIN_OVERSEER_INTERVAL_SECS)
}

/// Production entry point: read the real process environment.
pub fn overseer_interval_secs() -> u64 {
    resolve_interval_secs(|k| std::env::var(k).ok())
}

/// Recognise an explicit truthy env value. Case-insensitive; trims surrounding
/// whitespace. Anything else (including `0`/`false`/empty) is falsey.
fn is_truthy(v: &str) -> bool {
    let norm = v.trim().to_ascii_lowercase();
    matches!(norm.as_str(), "1" | "true" | "yes" | "on")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build an injectable env resolver from a fixed map — no `std::env` mutation.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn overseer_disabled_by_default_when_unset() {
        assert!(!overseer_enabled_from(env(&[])));
    }

    #[test]
    fn overseer_enabled_only_on_explicit_truthy_values() {
        for truthy in ["1", "true", "TRUE", "True", "yes", "YES", "on", "  on  "] {
            assert!(
                overseer_enabled_from(env(&[(OVERSEER_ENABLED_ENV, truthy)])),
                "{truthy:?} should enable the Overseer"
            );
        }
    }

    #[test]
    fn overseer_stays_off_for_falsey_or_garbage_values() {
        for falsey in ["0", "false", "no", "off", "", "  ", "maybe", "2"] {
            assert!(
                !overseer_enabled_from(env(&[(OVERSEER_ENABLED_ENV, falsey)])),
                "{falsey:?} must NOT enable the Overseer"
            );
        }
    }

    #[test]
    fn budget_falls_back_to_default_when_unset() {
        assert!(approx(
            resolve_daily_budget_usd(env(&[])),
            DEFAULT_DAILY_BUDGET_USD
        ));
    }

    #[test]
    fn budget_reads_explicit_value() {
        assert!(approx(
            resolve_daily_budget_usd(env(&[(DAILY_BUDGET_ENV, "750")])),
            750.0
        ));
        assert!(approx(
            resolve_daily_budget_usd(env(&[(DAILY_BUDGET_ENV, "  1234.5 ")])),
            1234.5
        ));
    }

    #[test]
    fn budget_falls_back_on_unparseable_empty_or_nonpositive() {
        for bad in ["abc", "", "  ", "0", "-5", "-0.01", "nan", "inf"] {
            assert!(
                approx(
                    resolve_daily_budget_usd(env(&[(DAILY_BUDGET_ENV, bad)])),
                    DEFAULT_DAILY_BUDGET_USD
                ),
                "{bad:?} must fall back to the default budget"
            );
        }
    }

    #[test]
    fn interval_defaults_when_unset() {
        assert_eq!(
            resolve_interval_secs(env(&[])),
            DEFAULT_OVERSEER_INTERVAL_SECS
        );
    }

    #[test]
    fn interval_reads_explicit_value_above_floor() {
        assert_eq!(
            resolve_interval_secs(env(&[(OVERSEER_INTERVAL_ENV, "1800")])),
            1800
        );
    }

    #[test]
    fn interval_clamps_to_floor() {
        // Below-floor / zero / garbage values never produce a hot loop.
        for below in ["1", "0", "59", "abc", ""] {
            assert!(
                resolve_interval_secs(env(&[(OVERSEER_INTERVAL_ENV, below)]))
                    >= MIN_OVERSEER_INTERVAL_SECS,
                "{below:?} must clamp to the floor"
            );
        }
        assert_eq!(
            resolve_interval_secs(env(&[(OVERSEER_INTERVAL_ENV, "1")])),
            MIN_OVERSEER_INTERVAL_SECS
        );
    }
}
