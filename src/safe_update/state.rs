//! Shared state-dir helpers for the safe-update orchestration.
//!
//! Centralising the on-disk schema (paths, JSON shapes, phase tags) keeps
//! the per-phase modules small and ensures the watchdog and operator CLIs
//! read the same files the orchestrator writes.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::errors::SafeUpdateError;

/// We keep at most this many `simard.bak.<utc>` files in `~/.simard/bin/`
/// so a long-running instance does not slowly fill the disk with old binaries.
pub const DEFAULT_BACKUP_RETENTION: usize = 5;

/// Phase tag stored in `upgrade-status.json`.
///
/// `Idle` is the implicit state when the file is absent; we never write it
/// but the operator CLI may report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradePhase {
    /// The orchestrator is in the middle of phases 1–3.
    InProgress,
    /// Pre-test refused the candidate; the orchestrator did not swap.
    PretestFailed,
    /// Swap completed; the new binary just exec()'d itself.
    ExecHandover,
    /// New binary completed validate_timeout_cycles cleanly.
    Validated,
    /// New binary did not validate within the budget; the watchdog will roll back.
    ValidateTimeout,
    /// Rollback restored the previous binary.
    RolledBack,
}

/// On-disk schema for `state_dir/upgrade-status.json`.
///
/// `serde(default)` on optional fields keeps the file readable across
/// version skew (e.g. an older operator binary inspecting a newer file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeStatus {
    pub phase: UpgradePhase,
    /// UTC ISO-8601 timestamp the phase was entered.
    pub started_at: String,
    /// Version of the binary the orchestrator is moving *to*.
    pub new_version: Option<String>,
    /// Version of the binary the orchestrator started from (for rollback).
    pub previous_version: Option<String>,
    /// Brief operator-friendly explanation; populated for failure phases.
    #[serde(default)]
    pub reason: Option<String>,
    /// Number of OODA cycles required before phase=validated.
    #[serde(default)]
    pub validate_required_cycles: Option<u32>,
    /// Cycles observed so far in validation mode.
    #[serde(default)]
    pub validate_cycles_seen: u32,
    /// Wall-clock budget (seconds) the new binary has to validate.
    #[serde(default)]
    pub validate_budget_seconds: Option<u64>,
}

impl UpgradeStatus {
    /// Build a status row tagged `exec_handover`.
    pub fn exec_handover(
        new_version: Option<String>,
        previous_version: Option<String>,
        validate_required_cycles: u32,
        validate_budget_seconds: u64,
    ) -> Self {
        Self {
            phase: UpgradePhase::ExecHandover,
            started_at: now_iso8601(),
            new_version,
            previous_version,
            reason: None,
            validate_required_cycles: Some(validate_required_cycles),
            validate_cycles_seen: 0,
            validate_budget_seconds: Some(validate_budget_seconds),
        }
    }

    /// Build a status row tagged `pretest_failed`.
    pub fn pretest_failed(code: Option<i32>, detail: String) -> Self {
        Self {
            phase: UpgradePhase::PretestFailed,
            started_at: now_iso8601(),
            new_version: None,
            previous_version: None,
            reason: Some(format!(
                "self-test exited {}: {}",
                code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                detail
            )),
            validate_required_cycles: None,
            validate_cycles_seen: 0,
            validate_budget_seconds: None,
        }
    }

    /// Build a status row tagged `rolled_back`.
    pub fn rolled_back(reason: String, restored_version: Option<String>) -> Self {
        Self {
            phase: UpgradePhase::RolledBack,
            started_at: now_iso8601(),
            new_version: restored_version,
            previous_version: None,
            reason: Some(reason),
            validate_required_cycles: None,
            validate_cycles_seen: 0,
            validate_budget_seconds: None,
        }
    }
}

/// Default state directory: `~/.simard/state/`. Falls back to `./.simard-state`
/// only if `$HOME` is unreadable, which lets tests stay hermetic.
pub fn default_state_dir() -> PathBuf {
    if let Some(home) = dirs_home() {
        home.join(".simard").join("state")
    } else {
        PathBuf::from(".simard-state")
    }
}

/// Path of the empty marker file that gates engineer dispatch.
pub fn draining_flag_path(state_dir: &Path) -> PathBuf {
    state_dir.join("draining.flag")
}

/// Path of `upgrade-status.json`.
pub fn status_path(state_dir: &Path) -> PathBuf {
    state_dir.join("upgrade-status.json")
}

/// `true` iff the draining flag exists. Cheap; safe to call from the
/// engineer dispatch hot path.
pub fn is_draining(state_dir: &Path) -> bool {
    draining_flag_path(state_dir).exists()
}

/// Read `upgrade-status.json`. Returns `Ok(None)` if the file is absent,
/// `Err(ValidateMalformed)` if the file exists but is unreadable as JSON,
/// `Err(ValidateReadFailed)` for filesystem errors.
pub fn read_status(state_dir: &Path) -> Result<Option<UpgradeStatus>, SafeUpdateError> {
    let path = status_path(state_dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|e| SafeUpdateError::ValidateReadFailed {
        reason: format!("read {}: {e}", path.display()),
    })?;
    let status: UpgradeStatus =
        serde_json::from_slice(&bytes).map_err(|e| SafeUpdateError::ValidateMalformed {
            reason: format!("{}: {e}", path.display()),
        })?;
    Ok(Some(status))
}

/// Write `upgrade-status.json` atomically (write-temp-then-rename).
pub fn write_status(state_dir: &Path, status: &UpgradeStatus) -> Result<(), SafeUpdateError> {
    fs::create_dir_all(state_dir).map_err(|e| SafeUpdateError::ValidateWriteFailed {
        reason: format!("mkdir {}: {e}", state_dir.display()),
    })?;
    let final_path = status_path(state_dir);
    let tmp_path = state_dir.join("upgrade-status.json.tmp");
    let body =
        serde_json::to_vec_pretty(status).map_err(|e| SafeUpdateError::ValidateWriteFailed {
            reason: format!("serialize: {e}"),
        })?;
    fs::write(&tmp_path, &body).map_err(|e| SafeUpdateError::ValidateWriteFailed {
        reason: format!("write {}: {e}", tmp_path.display()),
    })?;
    fs::rename(&tmp_path, &final_path).map_err(|e| SafeUpdateError::ValidateWriteFailed {
        reason: format!(
            "rename {} -> {}: {e}",
            tmp_path.display(),
            final_path.display()
        ),
    })?;
    Ok(())
}

/// Best-effort UTC ISO-8601 (e.g. `2025-05-11T21:34:56Z`).
pub(super) fn now_iso8601() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip_status_via_atomic_write() {
        let dir = tempdir().unwrap();
        let s = UpgradeStatus::exec_handover(Some("1.2.3".into()), Some("1.2.2".into()), 5, 600);
        write_status(dir.path(), &s).unwrap();
        let back = read_status(dir.path()).unwrap().unwrap();
        assert_eq!(back.phase, UpgradePhase::ExecHandover);
        assert_eq!(back.new_version.as_deref(), Some("1.2.3"));
        assert_eq!(back.previous_version.as_deref(), Some("1.2.2"));
        assert_eq!(back.validate_required_cycles, Some(5));
        assert_eq!(back.validate_budget_seconds, Some(600));
    }

    #[test]
    fn read_status_missing_returns_none() {
        let dir = tempdir().unwrap();
        let s = read_status(dir.path()).unwrap();
        assert!(s.is_none());
    }

    #[test]
    fn read_status_malformed_is_classified() {
        let dir = tempdir().unwrap();
        std::fs::write(status_path(dir.path()), b"{not json").unwrap();
        let err = read_status(dir.path()).unwrap_err();
        assert!(matches!(err, SafeUpdateError::ValidateMalformed { .. }));
    }

    #[test]
    fn pretest_failed_serialises_as_snake_case() {
        let s = UpgradeStatus::pretest_failed(Some(2), "boom".into());
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"pretest_failed\""), "json: {json}");
        assert!(json.contains("boom"));
    }

    #[test]
    fn is_draining_false_when_flag_missing() {
        let dir = tempdir().unwrap();
        assert!(!is_draining(dir.path()));
    }

    #[test]
    fn is_draining_true_when_flag_present() {
        let dir = tempdir().unwrap();
        std::fs::write(draining_flag_path(dir.path()), b"").unwrap();
        assert!(is_draining(dir.path()));
    }
}
