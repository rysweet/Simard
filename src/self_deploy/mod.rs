//! Reconcile-and-self-deploy: close the merged-but-not-running gap.
//!
//! Simard merges code to her own repository, then **build-from-source deploys
//! it into her own running daemon and verifies it is live** — or rolls back. A
//! merged self-change that is not running is treated as an open loop, not a
//! finished one.
//!
//! This module extends — it does not replace — [`crate::safe_update`],
//! [`crate::self_relaunch`], and [`crate::memory_backup`].
//!
//! **Implementation note.** The pure logic (drift detection, the health-report
//! AND-invariant, the orphan-match predicate, the orchestrator sequence) is
//! unit-tested hermetically. The OS-effectful operations — `cargo build`,
//! atomic swap, SIGTERM/SIGKILL signalling, systemd restart, the `/proc` scan,
//! the dual protective backup — are implemented for real; the orchestrator's
//! load-bearing sequence and its rollback tail are covered by injected
//! fake-effects tests, and the genuinely effectful end-to-end paths (real
//! build-from-source, a live systemd restart) are exercised by `#[ignore]`d
//! tests an operator runs against a live host, never in CI or the recipe.
//!
//! See `docs/concepts/reconcile-and-self-deploy.md` and
//! `docs/reference/self-deploy-api.md`.

pub mod backup;
pub mod drift;
pub mod health;
pub mod orchestrator;
pub mod orphan;
pub mod requeue;
pub mod restart;
pub mod source_prep;

pub use backup::{ProtectiveBackup, take_protective_backup};
pub use drift::{
    DeployDrift, DeploySource, GitDeploySource, ReconcileDetector, production_reconcile_detector,
};
pub use health::{
    BrainsLlmBackedProbe, GoalBoardIntactProbe, MemoryIntactProbe, NoQuarantineProbe,
    SelfHealthProbes, SelfHealthReport, VersionAdvancedProbe, run_self_health_probe,
};
pub use orchestrator::{DeploySourceKind, SelfDeployOrchestrator, SelfDeployOutcome};
pub use orphan::{
    OrphanEngineer, find_engineer_orphans, match_engineer_orphan, reap_engineer_orphans,
};
pub use requeue::ProdEngineerRequeue;
pub use restart::{DaemonRestarter, FakeRestarter, SystemdOrExecRestarter};
pub use source_prep::{
    GitSourcePreparer, SelfDeploySourcePreparer, self_deploy_src_dir, self_deploy_target_dir,
    validate_full_sha, validate_origin_transport,
};

#[cfg(test)]
mod tests_drift;
#[cfg(test)]
mod tests_health;
#[cfg(test)]
mod tests_orchestrator;
#[cfg(test)]
mod tests_orphan;
#[cfg(test)]
mod tests_restart;
#[cfg(test)]
mod tests_source_prep;
