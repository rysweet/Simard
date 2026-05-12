//! Brain-orchestrated safe self-update for Simard.
//!
//! Six-phase state machine that turns the raw `simard self-update` operator
//! command (download → self-test → exec) into a *safe* upgrade orchestrated
//! by the OODA brain:
//!
//! 1. **Drain**     — refuse new engineer dispatches; wait for in-flight to finish.
//! 2. **Snapshot**  — record current binary identity + back it up for rollback.
//! 3. **Pre-test**  — run the new binary's own `self-test` (gym starter suite).
//! 4. **Swap**      — atomically replace the install path; mark `exec_handover`.
//! 5. **Validate**  — after restart, complete N OODA cycles before declaring
//!    `validated`; otherwise mark `validate_timeout` for the watchdog.
//! 6. **Rollback**  — invoked by the watchdog or operator; restore from backup.
//!
//! Phases 1–4 run in the *outgoing* binary; phase 5 runs in the *incoming*
//! binary on first start; phase 6 runs in either binary on demand.
//!
//! All phase outputs land under `state_dir` (default `~/.simard/state/`):
//!
//! * `draining.flag`         — empty marker that gates engineer dispatch.
//! * `last-binary.json`      — snapshot record (path, sha256, mtime, version).
//! * `last-pretest.log`      — captured stdout+stderr of the pre-test self-test.
//! * `upgrade-status.json`   — current phase (`exec_handover` /
//!   `validated` / `validate_timeout` / `rolled_back` / `pretest_failed`).
//! * `upgrade-heartbeat.json`— heartbeat written each successful OODA cycle in
//!   validation mode.
//!
//! See `docs/safe-self-update.md` for the operator-facing description.

pub mod drain;
pub mod errors;
pub mod pretest;
pub mod rollback;
pub mod snapshot;
pub mod state;
pub mod swap;
pub mod validate;

#[cfg(test)]
mod tests_orchestrator;

use std::path::PathBuf;
use std::time::Duration;

pub use drain::{DrainOutcome, drain_to_quiescence, mark_draining, unmark_draining};
pub use errors::SafeUpdateError;
pub use pretest::{PretestOutcome, run_pretest};
pub use rollback::{RollbackOutcome, do_rollback};
pub use snapshot::{BinarySnapshot, take_snapshot};
pub use state::{
    DEFAULT_BACKUP_RETENTION, UpgradePhase, UpgradeStatus, default_state_dir, draining_flag_path,
    is_draining, read_status, status_path, write_status,
};
pub use swap::{SwapOutcome, do_swap};
pub use validate::{
    ValidateMode, default_install_bin, default_validate_timeout, enter_validation_if_needed,
    record_cycle, validation_required,
};

/// User-tunable safe-update knobs. Defaults here are deliberately conservative;
/// the brain prompt documents the four-part triggering doctrine separately.
#[derive(Debug, Clone)]
pub struct UpdateConfig {
    /// Minimum number of upstream commits since the running binary was built
    /// before the brain should consider an upgrade.
    pub min_commits_since_build: u32,
    /// Minimum minutes since the last update attempt (success *or* failure)
    /// before another attempt is allowed.
    pub min_minutes_since_last_attempt: u32,
    /// Maximum seconds to wait for in-flight engineer dispatches to drain
    /// before failing the orchestration with `DrainTimeout`.
    pub drain_timeout_seconds: u64,
    /// Maximum seconds to allow the new binary's `self-test` to run.
    pub pretest_timeout_seconds: u64,
    /// Minimum number of clean OODA cycles the incoming binary must complete
    /// before `validate` can succeed.
    pub validate_timeout_cycles: u32,
    /// Maximum wall-clock seconds the incoming binary may spend in validation
    /// mode before the watchdog rolls it back.
    pub validate_timeout_seconds: u64,
    /// Where to put `draining.flag`, `last-binary.json`, etc.
    pub state_dir: PathBuf,
    /// Where the OODA brain places engineer worktrees. Defaults to
    /// `~/.simard/engineer-worktrees/`. Tests can override this so the
    /// drain phase doesn't depend on the live filesystem.
    pub engineer_worktrees_root: Option<PathBuf>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            min_commits_since_build: 3,
            min_minutes_since_last_attempt: 30,
            drain_timeout_seconds: 300,
            pretest_timeout_seconds: 300,
            validate_timeout_cycles: 5,
            validate_timeout_seconds: 600,
            state_dir: default_state_dir(),
            engineer_worktrees_root: None,
        }
    }
}

/// Outcome of a successful (or aborted) safe-update orchestration.
#[derive(Debug, Clone)]
pub struct UpdateOutcome {
    pub drain: DrainOutcome,
    pub snapshot: BinarySnapshot,
    pub pretest: PretestOutcome,
    /// `Some` if the swap was attempted; `None` if pre-test refused.
    pub swap: Option<SwapOutcome>,
    pub elapsed: Duration,
}

/// Drives phases 1–4 (drain → snapshot → pre-test → swap+handover).
///
/// Phase 5 (validate) and phase 6 (rollback) are intentionally **not** part
/// of `run()` because:
///
/// * `swap` exec()s into the new binary on success — the call does not
///   return, so anything after it would be dead code.
/// * `validate` runs in the *new* binary's startup path
///   (see [`validate::enter_validation_if_needed`]).
/// * `rollback` is invoked by the operator (`simard rollback`) or the
///   watchdog (`simard rollback-watchdog`).
pub struct SafeUpdateOrchestrator {
    config: UpdateConfig,
    new_bin: PathBuf,
    install_path: PathBuf,
}

impl SafeUpdateOrchestrator {
    /// Construct an orchestrator. `new_bin` is the path to the already-
    /// downloaded candidate; `install_path` is where `simard` lives on disk
    /// and is what will be atomically replaced.
    pub fn new(config: UpdateConfig, new_bin: PathBuf, install_path: PathBuf) -> Self {
        Self {
            config,
            new_bin,
            install_path,
        }
    }

    /// Drive phases 1–4 in order. On success this **does not return** because
    /// `swap` exec()s into the new binary. On failure (any phase) returns
    /// the corresponding [`SafeUpdateError`] without leaving the install
    /// path in a half-replaced state.
    pub fn run(&self) -> Result<UpdateOutcome, SafeUpdateError> {
        let started = std::time::Instant::now();

        // Phase 1: drain.
        let drain = match &self.config.engineer_worktrees_root {
            Some(root) => drain::drain_to_quiescence_with_root(
                &self.config.state_dir,
                self.config.drain_timeout_seconds,
                root,
            )?,
            None => drain_to_quiescence(&self.config.state_dir, self.config.drain_timeout_seconds)?,
        };

        // Phase 2: snapshot the current binary for rollback.
        let snapshot = take_snapshot(&self.config.state_dir)?;

        // Phase 3: pre-test the candidate binary.
        let pretest = run_pretest(
            &self.new_bin,
            &self.config.state_dir,
            self.config.pretest_timeout_seconds,
        )?;
        if !pretest.passed {
            // Mark phase=pretest_failed so the brain doesn't retry immediately.
            let _ = write_status(
                &self.config.state_dir,
                &UpgradeStatus::pretest_failed(pretest.exit_code, pretest.detail.clone()),
            );
            return Err(SafeUpdateError::PretestSelfTestFailed {
                code: pretest.exit_code,
                detail: pretest.detail.clone(),
            });
        }

        // Phase 4: atomic swap + handover.
        let swap = do_swap(
            &self.new_bin,
            &self.install_path,
            &self.config.state_dir,
            &snapshot,
            self.config.validate_timeout_cycles,
            self.config.validate_timeout_seconds,
        )?;

        // Compose outcome (only reachable in tests where handover is stubbed).
        Ok(UpdateOutcome {
            drain,
            snapshot,
            pretest,
            swap: Some(swap),
            elapsed: started.elapsed(),
        })
    }
}
