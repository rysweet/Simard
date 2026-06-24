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
///
/// Equivalent to [`record_cycle_with_parity`] with no item-count check.
pub fn record_cycle(state_dir: &Path, now_unix: i64) -> Result<ValidateMode, SafeUpdateError> {
    record_cycle_with_parity(state_dir, now_unix, None)
}

/// Verdict of the #107 memory item-count parity check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParityVerdict {
    /// Parity holds, or cannot be judged (no baseline / no current count). The
    /// upgrade is allowed to proceed — the lib's store-open gate is the hard
    /// guarantee; this is defense in depth and never blocks on an *unknown* count.
    Ok,
    /// The post-upgrade count fell short of a non-zero pre-upgrade baseline — the
    /// #107 silent-empty-read signature (e.g. `pre > 0`, `post == 0`). The upgrade
    /// must NOT be validated.
    Shortfall { pre: u64, post: u64 },
}

/// Compare a recorded pre-upgrade item count against the freshly-read
/// post-upgrade count.
///
/// Returns [`ParityVerdict::Shortfall`] only when there is a **non-zero**
/// baseline AND the post count is strictly smaller (the #107 signature: a
/// populated store that read back empty, or any loss). A missing baseline or a
/// missing current count yields [`ParityVerdict::Ok`] — we never block an upgrade
/// on an *unknown* count.
pub fn memory_parity_verdict(pre: Option<u64>, current: Option<u64>) -> ParityVerdict {
    match (pre, current) {
        (Some(p), Some(c)) if p > 0 && c < p => ParityVerdict::Shortfall { pre: p, post: c },
        _ => ParityVerdict::Ok,
    }
}

/// Record one clean OODA cycle, additionally enforcing **#107 memory item-count
/// parity** before declaring the upgrade `validated`.
///
/// `current_item_count` is the cognitive-memory total the *incoming* binary reads
/// now; pass `None` to skip the parity check (then this behaves exactly like
/// [`record_cycle`]). When the Nth clean cycle would otherwise flip the phase to
/// `validated`, the recorded pre-upgrade baseline
/// ([`UpgradeStatus::pre_upgrade_item_count`](super::state::UpgradeStatus)) is
/// compared against `current_item_count`. On a
/// [`ParityVerdict::Shortfall`] the phase is forced to `validate_timeout` (with a
/// parity reason) instead of `validated`, so the watchdog rolls the upgrade back.
/// Clean OODA cycles alone are **not** evidence of a healthy upgrade — a silently
/// empty store passes them.
pub fn record_cycle_with_parity(
    state_dir: &Path,
    now_unix: i64,
    current_item_count: Option<u64>,
) -> Result<ValidateMode, SafeUpdateError> {
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
        // #107 parity gate: clean cycles are necessary but not sufficient. Refuse
        // to validate an upgrade whose post-swap item count fell short of the
        // recorded pre-upgrade baseline (the silent empty-read signature), forcing
        // a rollback instead of declaring the empty store healthy.
        if let ParityVerdict::Shortfall { pre, post } =
            memory_parity_verdict(status.pre_upgrade_item_count, current_item_count)
        {
            status.phase = UpgradePhase::ValidateTimeout;
            status.reason = Some(format!(
                "memory item-count parity FAILED after {} clean cycles: pre-upgrade {pre}, \
                 post-upgrade {post} (#107 silent empty-read signature) — refusing to validate, \
                 rolling back",
                status.validate_cycles_seen
            ));
            write_status(state_dir, &status)?;
            return Ok(ValidateMode::Timeout);
        }

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
        write_handover_status_with_pre_count(
            state,
            cycles_seen,
            required,
            budget,
            started_at,
            None,
        );
    }

    fn write_handover_status_with_pre_count(
        state: &Path,
        cycles_seen: u32,
        required: u32,
        budget: u64,
        started_at: &str,
        pre_upgrade_item_count: Option<u64>,
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
            pre_upgrade_item_count,
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

    // ----- #107 memory item-count parity gate -----

    #[test]
    fn memory_parity_verdict_cases() {
        // Shortfall: a non-zero baseline that reads back smaller (incl. empty).
        assert_eq!(
            memory_parity_verdict(Some(2813), Some(0)),
            ParityVerdict::Shortfall { pre: 2813, post: 0 }
        );
        assert_eq!(
            memory_parity_verdict(Some(2813), Some(2812)),
            ParityVerdict::Shortfall {
                pre: 2813,
                post: 2812
            }
        );
        // Parity holds (equal or grown), or no judgement possible.
        assert_eq!(
            memory_parity_verdict(Some(2813), Some(2813)),
            ParityVerdict::Ok
        );
        assert_eq!(
            memory_parity_verdict(Some(2813), Some(3000)),
            ParityVerdict::Ok
        );
        assert_eq!(memory_parity_verdict(None, Some(0)), ParityVerdict::Ok); // no baseline
        assert_eq!(memory_parity_verdict(Some(2813), None), ParityVerdict::Ok); // unknown current
        assert_eq!(memory_parity_verdict(Some(0), Some(0)), ParityVerdict::Ok); // empty before & after
    }

    #[test]
    fn parity_gate_blocks_validation_on_shortfall_and_rolls_back() {
        // A populated pre-upgrade store (2813) that the incoming binary reads
        // back as 0 must NOT validate, even after enough clean cycles — it must
        // force validate_timeout so the watchdog rolls back.
        let dir = tempdir().unwrap();
        super::super::drain::mark_draining(dir.path()).unwrap();
        write_handover_status_with_pre_count(
            dir.path(),
            0,
            1,
            600,
            "2025-05-11T12:00:00Z",
            Some(2813),
        );
        let t0 = unix(2025, 5, 11, 12, 0, 0);

        // The single required clean cycle is reached, but parity fails (0 < 2813).
        let r = record_cycle_with_parity(dir.path(), t0 + 1, Some(0)).unwrap();
        assert_eq!(r, ValidateMode::Timeout);

        let s = read_status(dir.path()).unwrap().unwrap();
        assert_eq!(s.phase, UpgradePhase::ValidateTimeout);
        assert!(
            s.reason.as_deref().unwrap().contains("parity FAILED"),
            "reason should name the parity failure, got: {:?}",
            s.reason
        );
        // The engineer-dispatch gate is NOT reopened on a failed validation.
        assert!(super::super::state::is_draining(dir.path()));
    }

    #[test]
    fn parity_gate_allows_validation_when_count_matches() {
        // A populated store that the incoming binary reads back at full parity
        // must validate as normal.
        let dir = tempdir().unwrap();
        super::super::drain::mark_draining(dir.path()).unwrap();
        write_handover_status_with_pre_count(
            dir.path(),
            0,
            1,
            600,
            "2025-05-11T12:00:00Z",
            Some(2813),
        );
        let t0 = unix(2025, 5, 11, 12, 0, 0);

        let r = record_cycle_with_parity(dir.path(), t0 + 1, Some(2813)).unwrap();
        assert_eq!(r, ValidateMode::Validated);
        let s = read_status(dir.path()).unwrap().unwrap();
        assert_eq!(s.phase, UpgradePhase::Validated);
        // Healthy upgrade reopens engineer dispatch.
        assert!(!super::super::state::is_draining(dir.path()));
    }

    #[test]
    fn record_cycle_without_parity_baseline_is_unchanged() {
        // No recorded baseline => the parity check is inert and the legacy
        // clean-cycles-only behaviour holds.
        let dir = tempdir().unwrap();
        write_handover_status(dir.path(), 0, 1, 600, "2025-05-11T12:00:00Z");
        let t0 = unix(2025, 5, 11, 12, 0, 0);
        // Even with a 0 current count, no baseline means no block.
        let r = record_cycle_with_parity(dir.path(), t0 + 1, Some(0)).unwrap();
        assert_eq!(r, ValidateMode::Validated);
    }
}
