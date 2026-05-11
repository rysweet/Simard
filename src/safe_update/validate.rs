//! Phase 5: post-restart validation.
//!
//! When a candidate exec()s into itself, it sees `phase=exec_handover` in
//! `state_dir/upgrade-status.json`. The new binary must then complete a
//! configurable number of clean OODA cycles within a wall-clock budget.
//! Each clean cycle calls [`record_cycle`], which:
//!
//! * Updates `validate_cycles_seen` in the status file.
//! * Writes a heartbeat to `state_dir/upgrade-heartbeat.json`.
//! * On the Nth cycle, flips phase to `validated` and removes
//!   `draining.flag` so engineer dispatch can resume.
//!
//! If the wall-clock budget is exceeded before the cycle target, [`record_cycle`]
//! returns [`ValidateMode::Timeout`] and updates phase to `validate_timeout`,
//! which the watchdog picks up and converts into a rollback.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::drain::unmark_draining;
use super::errors::SafeUpdateError;
use super::state::{UpgradePhase, now_iso8601, read_status, write_status};

/// What [`enter_validation_if_needed`] tells the OODA loop to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidateMode {
    /// No upgrade in progress; run normally.
    NotRequired,
    /// We are in validation mode, with this many cycles still required.
    InProgress { cycles_remaining: u32 },
    /// Validation already completed (`phase=validated`).
    Validated,
    /// Wall-clock budget exhausted; the watchdog will roll back.
    Timeout,
    /// Rollback already happened.
    RolledBack,
    /// Pre-test refused before we ever swapped.
    PretestFailed,
}

/// On-disk schema for `state_dir/upgrade-heartbeat.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeHeartbeat {
    pub last_cycle_at: String,
    pub cycles_seen: u32,
    pub remaining_seconds: i64,
}

/// Default budget when the status file does not specify one. Matches
/// [`super::UpdateConfig::validate_timeout_seconds`].
pub fn default_validate_timeout() -> u64 {
    600
}

/// `~/.simard/bin/simard` — the live install path used by [`super::do_swap`].
pub fn default_install_bin() -> PathBuf {
    super::snapshot::default_bin_dir().join("simard")
}

/// True iff `state_dir/upgrade-status.json` says we still owe validation
/// cycles (phase `exec_handover` with `cycles_seen < required`).
pub fn validation_required(state_dir: &Path) -> Result<bool, SafeUpdateError> {
    Ok(matches!(
        enter_validation_if_needed(state_dir)?,
        ValidateMode::InProgress { .. }
    ))
}

/// Inspect the current phase. Pure read; safe to call from the OODA
/// scheduler on every tick.
pub fn enter_validation_if_needed(state_dir: &Path) -> Result<ValidateMode, SafeUpdateError> {
    let Some(status) = read_status(state_dir)? else {
        return Ok(ValidateMode::NotRequired);
    };
    match status.phase {
        UpgradePhase::ExecHandover => {
            let required = status.validate_required_cycles.unwrap_or(5);
            let seen = status.validate_cycles_seen;
            if seen >= required {
                Ok(ValidateMode::Validated)
            } else {
                Ok(ValidateMode::InProgress {
                    cycles_remaining: required - seen,
                })
            }
        }
        UpgradePhase::Validated => Ok(ValidateMode::Validated),
        UpgradePhase::ValidateTimeout => Ok(ValidateMode::Timeout),
        UpgradePhase::RolledBack => Ok(ValidateMode::RolledBack),
        UpgradePhase::PretestFailed => Ok(ValidateMode::PretestFailed),
        UpgradePhase::InProgress => Ok(ValidateMode::NotRequired),
    }
}

/// Record one clean OODA cycle while in validation mode.
///
/// Idempotent: if the phase is no longer `exec_handover` (e.g. another
/// process already marked `validated`), this is a no-op.
///
/// `now_unix` is injected so tests can deterministically drive elapsed time.
pub fn record_cycle(state_dir: &Path, now_unix: i64) -> Result<ValidateMode, SafeUpdateError> {
    let Some(mut status) = read_status(state_dir)? else {
        return Ok(ValidateMode::NotRequired);
    };
    if !matches!(status.phase, UpgradePhase::ExecHandover) {
        return enter_validation_if_needed(state_dir);
    }

    // Wall-clock budget check first so a late cycle does not spuriously
    // report success after the budget already expired.
    let started = parse_iso8601_to_unix(&status.started_at);
    let budget = status
        .validate_budget_seconds
        .unwrap_or_else(default_validate_timeout) as i64;
    let elapsed = now_unix - started;
    let remaining = budget - elapsed;

    if remaining <= 0 {
        status.phase = UpgradePhase::ValidateTimeout;
        status.reason = Some(format!(
            "validate_timeout: {elapsed}s elapsed > {budget}s budget"
        ));
        write_status(state_dir, &status)?;
        return Ok(ValidateMode::Timeout);
    }

    status.validate_cycles_seen = status.validate_cycles_seen.saturating_add(1);
    let required = status.validate_required_cycles.unwrap_or(5);

    write_heartbeat(state_dir, status.validate_cycles_seen, remaining)?;

    if status.validate_cycles_seen >= required {
        status.phase = UpgradePhase::Validated;
        status.reason = Some(format!(
            "{} clean cycles within {}s",
            status.validate_cycles_seen, elapsed
        ));
        write_status(state_dir, &status)?;
        // The new binary is healthy: re-open the engineer-dispatch gate.
        unmark_draining(state_dir)?;
        return Ok(ValidateMode::Validated);
    }

    write_status(state_dir, &status)?;
    Ok(ValidateMode::InProgress {
        cycles_remaining: required - status.validate_cycles_seen,
    })
}

/// Force the phase to `validate_timeout` *now*, regardless of cycle count.
/// Used by the watchdog when it detects the new binary has stopped
/// emitting heartbeats.
pub fn force_validate_timeout(state_dir: &Path, reason: &str) -> Result<(), SafeUpdateError> {
    if let Some(mut status) = read_status(state_dir)? {
        status.phase = UpgradePhase::ValidateTimeout;
        status.reason = Some(reason.to_string());
        write_status(state_dir, &status)?;
    }
    Ok(())
}

fn write_heartbeat(
    state_dir: &Path,
    cycles_seen: u32,
    remaining_seconds: i64,
) -> Result<(), SafeUpdateError> {
    let path = state_dir.join("upgrade-heartbeat.json");
    let beat = UpgradeHeartbeat {
        last_cycle_at: now_iso8601(),
        cycles_seen,
        remaining_seconds,
    };
    let body =
        serde_json::to_vec_pretty(&beat).map_err(|e| SafeUpdateError::ValidateWriteFailed {
            reason: format!("serialize heartbeat: {e}"),
        })?;
    fs::write(&path, &body).map_err(|e| SafeUpdateError::ValidateWriteFailed {
        reason: format!("write {}: {e}", path.display()),
    })
}

fn parse_iso8601_to_unix(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_handover_status(
        state: &Path,
        cycles_seen: u32,
        required: u32,
        budget: u64,
        started_at: &str,
    ) {
        use super::super::state::UpgradeStatus;
        let s = UpgradeStatus {
            phase: UpgradePhase::ExecHandover,
            started_at: started_at.into(),
            new_version: Some("1.2.3".into()),
            previous_version: Some("1.2.2".into()),
            reason: None,
            validate_required_cycles: Some(required),
            validate_cycles_seen: cycles_seen,
            validate_budget_seconds: Some(budget),
        };
        write_status(state, &s).unwrap();
    }

    fn unix(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, min, sec)
            .unwrap()
            .and_utc()
            .timestamp()
    }

    #[test]
    fn enter_validation_returns_not_required_when_no_status() {
        let dir = tempdir().unwrap();
        assert_eq!(
            enter_validation_if_needed(dir.path()).unwrap(),
            ValidateMode::NotRequired
        );
    }

    #[test]
    fn record_cycle_increments_until_validated_then_clears_drain_flag() {
        let dir = tempdir().unwrap();
        // Engineer dispatch is currently gated.
        super::super::drain::mark_draining(dir.path()).unwrap();
        // Status: 0/3 cycles, 600s budget, started at the same instant we use.
        write_handover_status(dir.path(), 0, 3, 600, "2025-05-11T12:00:00Z");
        let t0 = unix(2025, 5, 11, 12, 0, 0);

        let r1 = record_cycle(dir.path(), t0 + 60).unwrap();
        assert_eq!(
            r1,
            ValidateMode::InProgress {
                cycles_remaining: 2
            }
        );
        let r2 = record_cycle(dir.path(), t0 + 120).unwrap();
        assert_eq!(
            r2,
            ValidateMode::InProgress {
                cycles_remaining: 1
            }
        );
        let r3 = record_cycle(dir.path(), t0 + 180).unwrap();
        assert_eq!(r3, ValidateMode::Validated);

        // Drain flag should be cleared after Validated.
        assert!(!super::super::state::is_draining(dir.path()));
        // Heartbeat written.
        assert!(dir.path().join("upgrade-heartbeat.json").exists());
        // Status is Validated.
        let s = read_status(dir.path()).unwrap().unwrap();
        assert_eq!(s.phase, UpgradePhase::Validated);
        assert_eq!(s.validate_cycles_seen, 3);
    }

    #[test]
    fn record_cycle_marks_timeout_when_budget_exhausted() {
        let dir = tempdir().unwrap();
        write_handover_status(dir.path(), 0, 5, 60, "2025-05-11T12:00:00Z");
        let t0 = unix(2025, 5, 11, 12, 0, 0);
        // 120s elapsed > 60s budget.
        let r = record_cycle(dir.path(), t0 + 120).unwrap();
        assert_eq!(r, ValidateMode::Timeout);
        let s = read_status(dir.path()).unwrap().unwrap();
        assert_eq!(s.phase, UpgradePhase::ValidateTimeout);
        assert!(s.reason.as_deref().unwrap().contains("validate_timeout"));
    }

    #[test]
    fn record_cycle_is_noop_after_already_validated() {
        let dir = tempdir().unwrap();
        write_handover_status(dir.path(), 0, 1, 600, "2025-05-11T12:00:00Z");
        let t0 = unix(2025, 5, 11, 12, 0, 0);
        record_cycle(dir.path(), t0 + 1).unwrap(); // -> Validated
        let r = record_cycle(dir.path(), t0 + 2).unwrap();
        assert_eq!(r, ValidateMode::Validated);
    }

    #[test]
    fn force_validate_timeout_flips_phase() {
        let dir = tempdir().unwrap();
        write_handover_status(dir.path(), 1, 5, 600, "2025-05-11T12:00:00Z");
        force_validate_timeout(dir.path(), "watchdog: no heartbeat in 90s").unwrap();
        let s = read_status(dir.path()).unwrap().unwrap();
        assert_eq!(s.phase, UpgradePhase::ValidateTimeout);
        assert!(s.reason.as_deref().unwrap().contains("watchdog"));
    }
}
