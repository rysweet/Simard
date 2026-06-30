//! Self-deploy orchestrator: drives the load-bearing sequence that turns a
//! merged self-change into a verified-running daemon, or rolls back.
//!
//! Order (load-bearing): build candidate from merged source → run gates +
//! candidate self-test → dual protective backup → drain → orphan-reap →
//! atomic swap → restart via [`DaemonRestarter`] → post-deploy health check →
//! rollback on a failed health check. Idempotent and loud.
//!
//! Extends — does not replace — the existing `safe_update` and `self_relaunch`
//! modules. See `docs/concepts/reconcile-and-self-deploy.md` and
//! `docs/reference/self-deploy-api.md#selfdeployorchestrator`.

use std::path::{Path, PathBuf};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::safe_update::{SafeUpdateError, UpdateConfig};

use super::backup::ProtectiveBackup;
use super::health::SelfHealthReport;
use super::restart::DaemonRestarter;
use super::source_prep::SelfDeploySourcePreparer;

/// Whether the candidate binary is built from merged source or downloaded as a
/// tagged release. Self-deploy of a merged-but-unreleased `main` commit uses
/// [`BuildFromSource`](DeploySourceKind::BuildFromSource).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DeploySourceKind {
    /// Build the candidate from the merged `main` source at the target commit.
    #[default]
    BuildFromSource,
    /// Download the candidate as a published, tagged release.
    ReleaseDownload,
}

/// Outcome of a successful self-deploy.
#[derive(Debug)]
pub struct SelfDeployOutcome {
    pub backup: ProtectiveBackup,
    pub reaped_orphans: usize,
    pub health: SelfHealthReport,
    pub restarter_kind: &'static str,
}

/// The effectful steps of a self-deploy, abstracted so the sequence is testable.
///
/// Production wires these to the real helpers (see [`ProdDeployEffects`]); tests
/// inject fakes to exercise ordering, the health gate, and the rollback tail
/// without building from source or restarting a daemon.
pub(crate) trait DeployEffects {
    /// Build the candidate binary from merged source; returns its path.
    fn build_candidate(&self) -> Result<PathBuf, SafeUpdateError>;
    /// Run the relaunch gates + candidate self-test against the candidate.
    fn gate_candidate(&self, candidate: &Path) -> Result<(), SafeUpdateError>;
    /// Read the live cognitive-memory fact count before any mutation.
    fn capture_baseline_facts(&self) -> Option<u64>;
    /// Take BOTH protective backups (memory + binary).
    fn take_backup(&self) -> Result<ProtectiveBackup, SafeUpdateError>;
    /// Drain in-flight engineer dispatches to quiescence.
    fn drain(&self) -> Result<(), SafeUpdateError>;
    /// Reap stale `engineer run` orphans bound to the old binary; returns count.
    fn reap_orphans(&self) -> Result<usize, SafeUpdateError>;
    /// Atomically replace the install path with the candidate.
    fn swap(&self, candidate: &Path) -> Result<(), SafeUpdateError>;
    /// Restart the daemon (systemd or injected fake).
    fn restart(&self) -> Result<(), SafeUpdateError>;
    /// Run the post-deploy health probe with the captured baseline.
    fn health_check(
        &self,
        baseline_facts: Option<u64>,
    ) -> Result<SelfHealthReport, SafeUpdateError>;
    /// Restore the previous binary and restart it (rollback).
    fn rollback(&self, reason: &str) -> Result<(), SafeUpdateError>;
    /// Human-readable restarter name for the outcome record.
    fn restarter_kind(&self) -> &'static str;
}

/// Run the load-bearing sequence over the injected effects. Shared by the
/// production `run()` and the hermetic fake-effects tests.
pub(crate) fn run_sequence<E: DeployEffects>(
    effects: &E,
) -> Result<SelfDeployOutcome, SafeUpdateError> {
    // 1) Build the candidate from merged source. Cheap-fail FIRST so a broken
    //    build never touches the daemon.
    let candidate = effects.build_candidate()?;

    // 2) Gate the candidate (relaunch gates + its own self-test).
    effects.gate_candidate(&candidate)?;

    // 3) Capture the live memory baseline, then take BOTH protective backups
    //    immediately before any daemon mutation.
    let baseline = effects.capture_baseline_facts();
    let backup = effects.take_backup()?;

    // 4) Drain in-flight engineers, then reap orphans holding the old inode.
    effects.drain()?;
    let reaped = effects.reap_orphans()?;

    // 5) Atomic swap, then restart.
    effects.swap(&candidate)?;
    if let Err(e) = effects.restart() {
        return rollback_then(effects, &format!("restart failed: {e}"));
    }

    // 6) Post-deploy health check; roll back on any unhealthy probe.
    let health = effects.health_check(baseline)?;
    if !health.healthy {
        return rollback_then(
            effects,
            &format!("health check failed: {}", summarize(&health)),
        );
    }

    Ok(SelfDeployOutcome {
        backup,
        reaped_orphans: reaped,
        health,
        restarter_kind: effects.restarter_kind(),
    })
}

/// Perform rollback and map the result to the terminal error: `RolledBack` on a
/// successful restore, `RollbackFailed` (critical) when rollback itself fails.
fn rollback_then<E: DeployEffects>(
    effects: &E,
    reason: &str,
) -> Result<SelfDeployOutcome, SafeUpdateError> {
    match effects.rollback(reason) {
        Ok(()) => Err(SafeUpdateError::RolledBack {
            reason: reason.to_string(),
        }),
        Err(e) => Err(SafeUpdateError::RollbackFailed {
            detail: format!("{reason}; rollback also failed: {e}"),
        }),
    }
}

/// Compact one-line summary of the unhealthy probes for the rollback reason.
fn summarize(report: &SelfHealthReport) -> String {
    let p = &report.probes;
    let mut bad = Vec::new();
    if !p.version_advanced.healthy {
        bad.push("version_advanced");
    }
    if !p.memory_intact.healthy {
        bad.push("memory_intact");
    }
    if !p.goal_board_intact.healthy {
        bad.push("goal_board_intact");
    }
    if !p.brains_llm_backed.healthy {
        bad.push("brains_llm_backed");
    }
    if !p.no_quarantine.healthy {
        bad.push("no_quarantine");
    }
    if bad.is_empty() {
        "all probes healthy".to_string()
    } else {
        bad.join(", ")
    }
}

/// Drives the self-deploy sequence. Extends `SafeUpdateOrchestrator` with
/// build-from-source, the dual backup, the orphan reaper, the injected
/// restarter, and the health-check/rollback tail.
pub struct SelfDeployOrchestrator {
    config: UpdateConfig,
    restarter: Box<dyn DaemonRestarter>,
    target_commit: String,
    install_path: PathBuf,
    /// Optional cwd-independent source preparer (issue #2467). When `None`
    /// (the default via [`SelfDeployOrchestrator::new`]) the legacy build path
    /// is used; when `Some` (via [`SelfDeployOrchestrator::with_source`]) the
    /// candidate is built from the fetched+checked-out merged commit into the
    /// persistent warm target dir. Additive — preserves the existing contract.
    build_source: Option<Box<dyn SelfDeploySourcePreparer>>,
}

impl SelfDeployOrchestrator {
    pub fn new(
        config: UpdateConfig,
        restarter: Box<dyn DaemonRestarter>,
        target_commit: String,
        install_path: PathBuf,
    ) -> Self {
        Self {
            config,
            restarter,
            target_commit,
            install_path,
            build_source: None,
        }
    }

    /// Like [`new`](Self::new) but injects a cwd-independent
    /// [`SelfDeploySourcePreparer`] (issue #2467) so the candidate is built
    /// from the fetched+checked-out merged commit — not the cwd HEAD — into the
    /// persistent warm target dir. Additive: `new` is unchanged and the default
    /// (no-source) build path is byte-for-byte preserved.
    pub fn with_source(
        config: UpdateConfig,
        restarter: Box<dyn DaemonRestarter>,
        target_commit: String,
        install_path: PathBuf,
        source: Box<dyn SelfDeploySourcePreparer>,
    ) -> Self {
        Self {
            config,
            restarter,
            target_commit,
            install_path,
            build_source: Some(source),
        }
    }

    /// The injected source preparer, if any. `None` for the legacy path.
    // Used by the orchestrator tests to assert the additive `with_source` seam;
    // `run()` consumes the field directly, so this accessor is test-only.
    #[allow(dead_code)]
    pub(crate) fn build_source(&self) -> Option<&dyn SelfDeploySourcePreparer> {
        self.build_source.as_deref()
    }

    /// Execute: build → gate → backup → drain → reap → swap → restart →
    /// health → rollback-on-failure. Idempotent and loud. Returns the outcome
    /// or the first [`SafeUpdateError`]. On a failed restart or health check,
    /// performs rollback and returns [`SafeUpdateError::RolledBack`] (or
    /// [`SafeUpdateError::RollbackFailed`] if rollback itself fails).
    pub fn run(&self) -> Result<SelfDeployOutcome, SafeUpdateError> {
        let effects = ProdDeployEffects {
            config: &self.config,
            restarter: self.restarter.as_ref(),
            target_commit: &self.target_commit,
            install_path: &self.install_path,
            build_source: self.build_source.as_deref(),
        };
        run_sequence(&effects)
    }
}

/// Production wiring of [`DeployEffects`] onto the real helpers. The live
/// cognitive store is opened from the resolved state root on demand for the
/// backup, baseline and health steps.
struct ProdDeployEffects<'a> {
    config: &'a UpdateConfig,
    restarter: &'a dyn DaemonRestarter,
    target_commit: &'a str,
    install_path: &'a Path,
    /// Cwd-independent source preparer (issue #2467). `Some` => build the
    /// fetched+checked-out merged commit into the persistent warm target dir;
    /// `None` => the legacy `build_canary` path (cwd checkout, cold temp dir).
    build_source: Option<&'a dyn SelfDeploySourcePreparer>,
}

impl ProdDeployEffects<'_> {
    /// Open the live cognitive store from the resolved state root.
    fn open_store(
        &self,
    ) -> Result<crate::cognitive_memory::LibraryCognitiveMemory, SafeUpdateError> {
        let root = crate::state_root::simard_state_root();
        crate::cognitive_memory::LibraryCognitiveMemory::open(&root).map_err(|e| {
            SafeUpdateError::HealthCheckFailed {
                report: format!("cannot open cognitive store at {}: {e}", root.display()),
            }
        })
    }
}

impl DeployEffects for ProdDeployEffects<'_> {
    fn build_candidate(&self) -> Result<PathBuf, SafeUpdateError> {
        // Issue #2467: with an injected source preparer, build the
        // fetched+checked-out merged commit (the SHA `--check` reports) into the
        // persistent warm target dir — cwd-independent and incremental. The
        // legacy path (no source) is byte-for-byte preserved.
        if let Some(source) = self.build_source {
            let warm = super::source_prep::self_deploy_target_dir();
            return super::source_prep::prepare_and_build(source, self.target_commit, &warm);
        }
        crate::self_relaunch::build_canary(&crate::self_relaunch::RelaunchConfig::default())
            .map_err(|e| SafeUpdateError::BuildFailed {
                detail: e.to_string(),
            })
    }

    fn gate_candidate(&self, candidate: &Path) -> Result<(), SafeUpdateError> {
        // Candidate self-test (gym starter) via the existing pre-test phase.
        let pretest = crate::safe_update::run_pretest(
            candidate,
            &self.config.state_dir,
            self.config.pretest_timeout_seconds,
        )?;
        if !pretest.passed {
            return Err(SafeUpdateError::GateFailed {
                gate: "self-test".to_string(),
                detail: pretest.detail,
            });
        }
        Ok(())
    }

    fn capture_baseline_facts(&self) -> Option<u64> {
        self.open_store()
            .ok()
            .and_then(|mem| mem.get_statistics().ok())
            .map(|s| s.total())
    }

    fn take_backup(&self) -> Result<ProtectiveBackup, SafeUpdateError> {
        let mem = self.open_store()?;
        super::backup::take_protective_backup(&mem, self.install_path, &self.config.state_dir)
    }

    fn drain(&self) -> Result<(), SafeUpdateError> {
        match &self.config.engineer_worktrees_root {
            Some(root) => crate::safe_update::drain::drain_to_quiescence_with_root(
                &self.config.state_dir,
                self.config.drain_timeout_seconds,
                root,
            )
            .map(|_| ()),
            None => crate::safe_update::drain_to_quiescence(
                &self.config.state_dir,
                self.config.drain_timeout_seconds,
            )
            .map(|_| ()),
        }
    }

    fn reap_orphans(&self) -> Result<usize, SafeUpdateError> {
        let self_pid = std::process::id() as i32;
        let orphans = super::orphan::find_engineer_orphans(self.install_path, self_pid, None)
            .map_err(|e| SafeUpdateError::SwapFailed {
                reason: format!("orphan scan failed: {e}"),
            })?;
        match super::orphan::reap_engineer_orphans(&orphans, self.config.orphan_kill_grace_seconds)
        {
            Ok(n) => Ok(n),
            Err(_) => Err(SafeUpdateError::OrphanReapTimeout {
                pid: orphans.first().map(|o| o.pid).unwrap_or(0),
            }),
        }
    }

    fn swap(&self, candidate: &Path) -> Result<(), SafeUpdateError> {
        crate::safe_update::swap::atomic_install(candidate, self.install_path).map(|_| ())
    }

    fn restart(&self) -> Result<(), SafeUpdateError> {
        self.restarter
            .restart()
            .map_err(|e| SafeUpdateError::SwapHandoverFailed {
                reason: e.to_string(),
            })
    }

    fn health_check(
        &self,
        baseline_facts: Option<u64>,
    ) -> Result<SelfHealthReport, SafeUpdateError> {
        let mem = self.open_store()?;
        // Count brain parse-failures only from this deploy onward.
        let window_start = chrono::Utc::now();
        super::health::run_self_health_probe(
            &mem,
            self.target_commit,
            baseline_facts,
            self.config.memory_count_tolerance,
            window_start,
        )
        .map_err(|e| SafeUpdateError::HealthCheckFailed {
            report: e.to_string(),
        })
    }

    fn rollback(&self, reason: &str) -> Result<(), SafeUpdateError> {
        crate::safe_update::do_rollback(&self.config.state_dir, self.install_path, reason, None)
            .map(|_| ())
    }

    fn restarter_kind(&self) -> &'static str {
        self.restarter.kind()
    }
}
