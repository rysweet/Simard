//! Centralized cargo parallelism limit to prevent OOM (issue #2199).
//!
//! All Simard-spawned `cargo build` / `cargo test` invocations must use
//! this limit via the `CARGO_BUILD_JOBS` environment variable.
//!
//! Operators can override the default by setting `SIMARD_CARGO_JOBS`.
//! The value must be a positive integer; invalid values fall back to
//! the default of 2.

const DEFAULT_CARGO_JOBS: &str = "2";

/// Read `SIMARD_CARGO_JOBS` from the process environment and return
/// a validated string suitable for `CARGO_BUILD_JOBS`.
pub fn cargo_jobs() -> String {
    cargo_jobs_from(std::env::var("SIMARD_CARGO_JOBS").ok().as_deref())
}

/// Pure helper: validate an optional override value and return a string
/// suitable for `CARGO_BUILD_JOBS`. Returns `DEFAULT_CARGO_JOBS` when
/// the input is `None`, empty, non-numeric, zero, or negative.
///
/// This function does not touch `std::env`, so it is safe to use in
/// pure contexts (e.g., `compute_tmux_env`).
pub fn cargo_jobs_from(override_value: Option<&str>) -> String {
    match override_value {
        Some(s) if !s.trim().is_empty() => match s.trim().parse::<i32>() {
            Ok(n) if n > 0 => n.to_string(),
            _ => DEFAULT_CARGO_JOBS.to_string(),
        },
        _ => DEFAULT_CARGO_JOBS.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_when_none() {
        assert_eq!(cargo_jobs_from(None), "2");
    }

    #[test]
    fn default_when_empty() {
        assert_eq!(cargo_jobs_from(Some("")), "2");
    }

    #[test]
    fn default_when_whitespace() {
        assert_eq!(cargo_jobs_from(Some("  ")), "2");
    }

    #[test]
    fn default_when_non_numeric() {
        assert_eq!(cargo_jobs_from(Some("abc")), "2");
    }

    #[test]
    fn default_when_zero() {
        assert_eq!(cargo_jobs_from(Some("0")), "2");
    }

    #[test]
    fn default_when_negative() {
        assert_eq!(cargo_jobs_from(Some("-1")), "2");
    }

    #[test]
    fn accepts_valid_positive_integer() {
        assert_eq!(cargo_jobs_from(Some("4")), "4");
        assert_eq!(cargo_jobs_from(Some("1")), "1");
        assert_eq!(cargo_jobs_from(Some("8")), "8");
    }

    #[test]
    fn trims_whitespace_around_valid_value() {
        assert_eq!(cargo_jobs_from(Some(" 3 ")), "3");
    }

    #[test]
    fn default_when_float() {
        assert_eq!(cargo_jobs_from(Some("2.5")), "2");
    }
}
