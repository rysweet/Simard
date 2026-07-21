//! M3 — guarded (HIGH-RISK, opt-in) deploy via canary gates + a deploy gate that
//! refuses the dangerous shapes, plus the mandatory operator notification on
//! deploy.
//!
//! Reuse (design doc §capability table):
//! - Canary: `self_relaunch::{build_canary, verify_canary, all_gates_passed,
//!   default_gates}` (`src/self_relaunch/*`).
//! - Deploy: `self_deploy::orchestrator::SelfDeployOrchestrator::run`
//!   (`src/self_deploy/orchestrator.rs:229`).
//! - Running commit marker: `env!("SIMARD_GIT_HASH")` (matches
//!   `self_deploy::health`).
//!
//! Operator hard-gates encoded:
//! - `Deploy` stays HIGH-RISK and opt-in (`AutonomyGate.allow_high_risk`,
//!   default `false`) — enforced by [`guardrails`](crate::overseer::guardrails),
//!   not here.
//! - The **deploy gate** ([`evaluate_deploy_gate`]) refuses a no-op deploy
//!   (target == running), a **rollback** (target is an ancestor of running), a
//!   **red canary** (gates failed), and a **crash-loop** (elevated restart
//!   churn). A refused deploy notifies the operator and surfaces an error — it
//!   does not mutate the binary.
//! - Every deploy ATTEMPT fires [`NotifyOperator`](crate::overseer::notify) on
//!   both channels: a completed deploy sends a `deploy` notice; a gate refusal
//!   or a failed binary swap sends a `deploy-refused` notice (#2590), so the
//!   operator is never blind to an aborted autonomous deploy.
//! - The deployer never touches `~/.simard/worktrees`: it operates only on the
//!   canary target dir and the install path.

use crate::overseer::capabilities::{DeployReport, Deployer, OverseerError};
use crate::overseer::notify::{DualChannelNotifier, OperatorNotification};

/// Restart churn at/above which a deploy is refused as a suspected crash-loop
/// (deploying into an unstable process makes it worse — Bainbridge's irony).
pub const CRASH_LOOP_CHURN_THRESHOLD: u64 = 3;

/// The inputs the deploy gate judges. Assembled by [`GuardedDeployer`] from the
/// running-commit marker, the canary result, a git-ancestry check, and observed
/// restart churn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployContext {
    pub running_commit: String,
    pub target_commit: String,
    /// True iff `target_commit` is an ancestor of `running_commit` (a rollback).
    pub target_is_ancestor_of_running: bool,
    /// Did every canary gate pass?
    pub canary_passed: bool,
    /// Observed daemon restart churn over the recent window.
    pub recent_restart_churn: u64,
}

/// Why a deploy was refused. Each variant is a dangerous shape the operator's
/// manual discipline avoids and the unattended loop must too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeployRefusal {
    /// Target is already the running commit — nothing to do.
    NoOp,
    /// Target is older than (an ancestor of) the running commit.
    Rollback,
    /// The canary gates did not all pass.
    RedCanary,
    /// Restart churn suggests a crash-loop; deploying would worsen it.
    CrashLoop { churn: u64 },
}

impl std::fmt::Display for DeployRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoOp => write!(f, "no-op deploy (target == running commit)"),
            Self::Rollback => write!(f, "rollback refused (target is older than running)"),
            Self::RedCanary => write!(f, "red canary (one or more gates failed)"),
            Self::CrashLoop { churn } => {
                write!(
                    f,
                    "crash-loop suspected (restart churn {churn}) — not deploying"
                )
            }
        }
    }
}

/// The deploy gate (pure). Refuses no-op, rollback, red-canary, and crash-loop.
pub fn evaluate_deploy_gate(ctx: &DeployContext) -> Result<(), DeployRefusal> {
    if commits_equivalent(&ctx.running_commit, &ctx.target_commit) {
        return Err(DeployRefusal::NoOp);
    }
    if ctx.target_is_ancestor_of_running {
        return Err(DeployRefusal::Rollback);
    }
    if !ctx.canary_passed {
        return Err(DeployRefusal::RedCanary);
    }
    if ctx.recent_restart_churn >= CRASH_LOOP_CHURN_THRESHOLD {
        return Err(DeployRefusal::CrashLoop {
            churn: ctx.recent_restart_churn,
        });
    }
    Ok(())
}

/// Two commit hashes name the same commit if equal or one is a prefix of the
/// other (short vs long hash). Mirrors `self_deploy::health::commits_compatible`.
fn commits_equivalent(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a == b || a.starts_with(&b) || b.starts_with(&a)
}

// ─────────────────────────── injected seams ────────────────────────────────

/// Result of building + verifying the canary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanaryResult {
    pub passed: bool,
    pub detail: String,
}

/// Build + verify the canary binary for a target. Real impl reuses
/// `self_relaunch::{build_canary, verify_canary, all_gates_passed}`; tests fake it.
pub trait CanaryRunner {
    fn run_canary(&self, target_commit: &str) -> Result<CanaryResult, OverseerError>;
}

/// Perform the actual binary swap for a target, returning the deployed commit.
/// Real impl wraps `SelfDeployOrchestrator::run`; tests fake it. (HIGH-RISK, so
/// the real wiring is supplied explicitly by the operator when opting in.)
pub trait BinaryDeployer {
    fn deploy_binary(&self, target_commit: &str) -> Result<String, OverseerError>;
}

/// Answer "is `ancestor` an ancestor of `descendant`?" — reused to detect a
/// rollback. Real impl shells `git merge-base --is-ancestor`; tests fake it.
pub trait AncestryOracle {
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, OverseerError>;
}

/// Real ancestry oracle over `git merge-base --is-ancestor` in `repo_dir`.
pub struct GitAncestryOracle {
    pub repo_dir: std::path::PathBuf,
}

impl AncestryOracle for GitAncestryOracle {
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, OverseerError> {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repo_dir)
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .status()
            .map_err(|e| OverseerError::Capability {
                what: "git.merge-base",
                detail: e.to_string(),
            })?;
        // Exit 0 → is an ancestor; exit 1 → not; other → error.
        match status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            other => Err(OverseerError::Capability {
                what: "git.merge-base",
                detail: format!("unexpected exit {other:?}"),
            }),
        }
    }
}

// ─────────────────────────── the adapter ───────────────────────────────────

/// The guarded [`Deployer`]. Composes the canary, deploy gate, binary swap, and
/// mandatory operator notification. HIGH-RISK; only run when the operator opts
/// in via the `AutonomyGate`.
pub struct GuardedDeployer {
    canary: Box<dyn CanaryRunner>,
    deployer: Box<dyn BinaryDeployer>,
    ancestry: Box<dyn AncestryOracle>,
    notifier: DualChannelNotifier,
    running_commit: String,
    recent_restart_churn: u64,
    repo: String,
}

impl GuardedDeployer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        canary: Box<dyn CanaryRunner>,
        deployer: Box<dyn BinaryDeployer>,
        ancestry: Box<dyn AncestryOracle>,
        notifier: DualChannelNotifier,
        running_commit: String,
        recent_restart_churn: u64,
        repo: String,
    ) -> Self {
        Self {
            canary,
            deployer,
            ancestry,
            notifier,
            running_commit,
            recent_restart_churn,
            repo,
        }
    }

    /// The compile-time running commit (matches `self_deploy::health`).
    pub fn running_commit_marker() -> &'static str {
        env!("SIMARD_GIT_HASH")
    }
}

impl Deployer for GuardedDeployer {
    fn deploy(&self, commit: &str) -> Result<DeployReport, OverseerError> {
        let running = self.running_commit.clone();

        // Build + verify the canary first (its result feeds the gate).
        let canary = self.canary.run_canary(commit)?;

        // A rollback check only matters when the target differs from running.
        let is_ancestor = if commits_equivalent(&running, commit) {
            false
        } else {
            self.ancestry.is_ancestor(commit, &running)?
        };

        let ctx = DeployContext {
            running_commit: running.clone(),
            target_commit: commit.to_string(),
            target_is_ancestor_of_running: is_ancestor,
            canary_passed: canary.passed,
            recent_restart_churn: self.recent_restart_churn,
        };
        // Safety rail (#2590): notify the operator on EVERY deploy attempt,
        // including a gate REFUSAL — a refused deploy still leaves merged work
        // undeployed, which the operator must see. The notification is
        // best-effort here (a refusal already surfaces as an Err); the mandatory
        // dispatch assertion below guards the swap path.
        if let Err(refusal) = evaluate_deploy_gate(&ctx) {
            let notification = OperatorNotification::deploy_refused(
                commit,
                &running,
                &self.repo,
                &refusal.to_string(),
            );
            let _ = self.notifier.notify(&notification);
            return Err(OverseerError::Capability {
                what: "deploy_gate",
                detail: refusal.to_string(),
            });
        }

        // Gate passed — perform the binary swap. A failed swap is also an
        // operator-visible deploy attempt: notify before surfacing the error.
        let deployed = match self.deployer.deploy_binary(commit) {
            Ok(deployed) => deployed,
            Err(e) => {
                let notification = OperatorNotification::deploy_refused(
                    commit,
                    &running,
                    &self.repo,
                    &format!("binary swap failed: {e}"),
                );
                let _ = self.notifier.notify(&notification);
                return Err(e);
            }
        };

        // Mandatory: notify the operator on both channels.
        let notification = OperatorNotification::deploy(
            &deployed,
            &running,
            &self.repo,
            &format!("canary {} ({})", pass_word(canary.passed), canary.detail),
        );
        let report = self.notifier.notify(&notification);
        debug_assert!(
            report.dispatched(),
            "deploy completed without a dispatched operator notification"
        );

        Ok(DeployReport {
            deployed_commit: deployed,
            gates_passed: true,
        })
    }

    fn deployed_commit(&self) -> Result<String, OverseerError> {
        Ok(self.running_commit.clone())
    }
}

fn pass_word(passed: bool) -> &'static str {
    if passed { "green" } else { "red" }
}

// ─────────────────────────── production seams (#2590) ──────────────────────

/// Production [`CanaryRunner`]: build the candidate from merged source and run
/// the full relaunch gate sequence (Smoke → UnitTest → GymBaseline → RpcHealth)
/// via `self_relaunch::{build_canary, verify_canary, all_gates_passed}` — the
/// SAME canary the operator relaunch path uses. A build or verify FAILURE is a
/// RED canary (`passed: false`), NOT a hard error, so the deploy gate refuses it
/// (`RedCanary`) and nothing is swapped — fail-closed: an unverifiable candidate
/// never ships.
pub struct ProdCanaryRunner;

impl CanaryRunner for ProdCanaryRunner {
    fn run_canary(&self, _target_commit: &str) -> Result<CanaryResult, OverseerError> {
        use crate::self_relaunch::{
            RelaunchConfig, all_gates_passed, build_canary, default_gates, verify_canary,
        };
        let config = RelaunchConfig::default();
        let candidate = match build_canary(&config) {
            Ok(path) => path,
            Err(e) => {
                return Ok(CanaryResult {
                    passed: false,
                    detail: format!("canary build failed: {e}"),
                });
            }
        };
        let gates = default_gates();
        let results = match verify_canary(&candidate, &gates, &config) {
            Ok(r) => r,
            Err(e) => {
                return Ok(CanaryResult {
                    passed: false,
                    detail: format!("canary verify failed: {e}"),
                });
            }
        };
        let passed = all_gates_passed(&results);
        let ok = results.iter().filter(|r| r.passed).count();
        Ok(CanaryResult {
            passed,
            detail: format!("{ok}/{} gates", results.len()),
        })
    }
}

/// Production [`BinaryDeployer`]: the ACTUAL swap runs through
/// [`SelfDeployOrchestrator`](crate::self_deploy::SelfDeployOrchestrator) — the
/// EXACT build-from-source → gate → dual backup → drain → orphan-reap → atomic
/// swap → restart → health-check → rollback path `simard self-deploy` uses.
/// Reusing it verbatim keeps the autonomous deploy's swap/rollback/reap
/// byte-for-byte identical to the operator path (no second, divergent deploy
/// engine). A failed run surfaces as an error; on a restart/health failure the
/// orchestrator has ALREADY rolled back to the preserved prior binary.
pub struct OrchestratedBinaryDeployer;

impl BinaryDeployer for OrchestratedBinaryDeployer {
    fn deploy_binary(&self, target_commit: &str) -> Result<String, OverseerError> {
        use crate::self_deploy::{
            GitSourcePreparer, SelfDeployOrchestrator, SystemdOrExecRestarter,
        };
        let install_path = std::env::current_exe().map_err(|e| OverseerError::Capability {
            what: "self_deploy.current_exe",
            detail: e.to_string(),
        })?;
        let orchestrator = SelfDeployOrchestrator::with_source(
            crate::safe_update::UpdateConfig::default(),
            Box::new(SystemdOrExecRestarter::new()),
            target_commit.to_string(),
            install_path,
            Box::new(GitSourcePreparer::new()),
        );
        orchestrator
            .run()
            .map(|_outcome| target_commit.to_string())
            .map_err(|e| OverseerError::Capability {
                what: "self_deploy",
                detail: e.to_string(),
            })
    }
}

/// Assemble the PRODUCTION guarded deployer (#2590): the real canary + the
/// orchestrator swap + a git ancestry oracle rooted at the daemon's repo + the
/// mandatory dual-channel operator notifier. `running_commit` is the binary's
/// embedded build hash; `recent_restart_churn` is read LIVE at assembly time (the
/// daemon rebuilds the Overseer every tick) so the gate's crash-loop refusal sees
/// current churn. `repo` labels the operator notification.
pub fn production_guarded_deployer(
    repo_dir: std::path::PathBuf,
    recent_restart_churn: u64,
    repo: String,
) -> GuardedDeployer {
    GuardedDeployer::new(
        Box::new(ProdCanaryRunner),
        Box::new(OrchestratedBinaryDeployer),
        Box::new(GitAncestryOracle { repo_dir }),
        DualChannelNotifier::from_env(),
        GuardedDeployer::running_commit_marker().to_string(),
        recent_restart_churn,
        repo,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overseer::notify::{ChannelDelivery, NotifyChannel, OperatorNotification};
    use std::sync::{Arc, Mutex};

    // ── pure gate ────────────────────────────────────────────────────────────

    fn ctx() -> DeployContext {
        DeployContext {
            running_commit: "aaaaaaaaaaaa".to_string(),
            target_commit: "bbbbbbbbbbbb".to_string(),
            target_is_ancestor_of_running: false,
            canary_passed: true,
            recent_restart_churn: 0,
        }
    }

    #[test]
    fn gate_allows_a_clean_forward_deploy() {
        assert!(evaluate_deploy_gate(&ctx()).is_ok());
    }

    #[test]
    fn gate_refuses_no_op() {
        let mut c = ctx();
        c.target_commit = c.running_commit.clone();
        assert_eq!(evaluate_deploy_gate(&c), Err(DeployRefusal::NoOp));
        // Short-vs-long hash is also a no-op.
        let mut c = ctx();
        c.running_commit = "aaaaaaaaaaaa1111".to_string();
        c.target_commit = "aaaaaaaaaaaa".to_string();
        assert_eq!(evaluate_deploy_gate(&c), Err(DeployRefusal::NoOp));
    }

    #[test]
    fn gate_refuses_rollback() {
        let mut c = ctx();
        c.target_is_ancestor_of_running = true;
        assert_eq!(evaluate_deploy_gate(&c), Err(DeployRefusal::Rollback));
    }

    #[test]
    fn gate_refuses_red_canary() {
        let mut c = ctx();
        c.canary_passed = false;
        assert_eq!(evaluate_deploy_gate(&c), Err(DeployRefusal::RedCanary));
    }

    #[test]
    fn gate_refuses_crash_loop() {
        let mut c = ctx();
        c.recent_restart_churn = CRASH_LOOP_CHURN_THRESHOLD;
        assert_eq!(
            evaluate_deploy_gate(&c),
            Err(DeployRefusal::CrashLoop {
                churn: CRASH_LOOP_CHURN_THRESHOLD
            })
        );
    }

    // ── the adapter ──────────────────────────────────────────────────────────

    struct FakeCanary(bool);
    impl CanaryRunner for FakeCanary {
        fn run_canary(&self, _t: &str) -> Result<CanaryResult, OverseerError> {
            Ok(CanaryResult {
                passed: self.0,
                detail: "4/4 gates".to_string(),
            })
        }
    }

    struct FakeAncestry(bool);
    impl AncestryOracle for FakeAncestry {
        fn is_ancestor(&self, _a: &str, _d: &str) -> Result<bool, OverseerError> {
            Ok(self.0)
        }
    }

    struct Capture(Arc<Mutex<Vec<OperatorNotification>>>);
    impl NotifyChannel for Capture {
        fn name(&self) -> &str {
            "capture"
        }
        fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
            self.0.lock().unwrap().push(n.clone());
            ChannelDelivery::Sent
        }
    }

    #[allow(clippy::type_complexity)]
    fn deployer(
        canary_passed: bool,
        is_ancestor: bool,
        churn: u64,
    ) -> (
        GuardedDeployer,
        Arc<Mutex<usize>>,
        Arc<Mutex<Vec<OperatorNotification>>>,
    ) {
        let deployed = Arc::new(Mutex::new(0));
        let seen = Arc::new(Mutex::new(vec![]));
        let notifier = DualChannelNotifier::new(vec![Box::new(Capture(seen.clone()))]);
        let gd = GuardedDeployer::new(
            Box::new(FakeCanary(canary_passed)),
            Box::new(FakeDeployerShared(deployed.clone())),
            Box::new(FakeAncestry(is_ancestor)),
            notifier,
            "aaaaaaaaaaaa".to_string(),
            churn,
            "rysweet/Simard".to_string(),
        );
        (gd, deployed, seen)
    }

    struct FakeDeployerShared(Arc<Mutex<usize>>);
    impl BinaryDeployer for FakeDeployerShared {
        fn deploy_binary(&self, target: &str) -> Result<String, OverseerError> {
            *self.0.lock().unwrap() += 1;
            Ok(target.to_string())
        }
    }

    #[test]
    fn deploy_succeeds_and_notifies_on_clean_forward() {
        let (gd, deployed, seen) = deployer(true, false, 0);
        let report = gd.deploy("bbbbbbbbbbbb").expect("deploy");
        assert!(report.gates_passed);
        assert_eq!(report.deployed_commit, "bbbbbbbbbbbb");
        assert_eq!(*deployed.lock().unwrap(), 1, "binary swapped once");
        assert_eq!(seen.lock().unwrap().len(), 1, "operator notified on deploy");
        assert_eq!(seen.lock().unwrap()[0].kind, "deploy");
    }

    #[test]
    fn deploy_refuses_no_op_notifies_without_swapping() {
        let (gd, deployed, seen) = deployer(true, false, 0);
        // Target == running → no-op.
        let err = gd.deploy("aaaaaaaaaaaa").unwrap_err();
        assert!(format!("{err}").contains("no-op"));
        assert_eq!(*deployed.lock().unwrap(), 0, "no binary swap on refusal");
        // Safety rail (#2590): a refusal still notifies the operator so merged
        // work left undeployed is never silent.
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "operator notified on refusal"
        );
        assert_eq!(seen.lock().unwrap()[0].kind, "deploy-refused");
    }

    #[test]
    fn deploy_refuses_rollback() {
        let (gd, deployed, _) = deployer(true, true, 0);
        assert!(format!("{}", gd.deploy("bbbbbbbbbbbb").unwrap_err()).contains("rollback"));
        assert_eq!(*deployed.lock().unwrap(), 0);
    }

    #[test]
    fn deploy_refuses_red_canary() {
        let (gd, deployed, _) = deployer(false, false, 0);
        assert!(format!("{}", gd.deploy("bbbbbbbbbbbb").unwrap_err()).contains("red canary"));
        assert_eq!(*deployed.lock().unwrap(), 0);
    }

    #[test]
    fn deploy_refuses_crash_loop() {
        let (gd, deployed, _) = deployer(true, false, CRASH_LOOP_CHURN_THRESHOLD);
        assert!(format!("{}", gd.deploy("bbbbbbbbbbbb").unwrap_err()).contains("crash-loop"));
        assert_eq!(*deployed.lock().unwrap(), 0);
    }

    #[test]
    fn deployed_commit_reports_running() {
        let (gd, _, _) = deployer(true, false, 0);
        assert_eq!(gd.deployed_commit().unwrap(), "aaaaaaaaaaaa");
    }
}
