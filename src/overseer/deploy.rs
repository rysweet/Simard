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
//!   both channels: a gate-passing attempt sends a pre-swap `deploy-starting`
//!   notice before invoking the process-replacing swap; a completed deploy sends
//!   a `deploy` notice when the swap path returns; a gate refusal or a failed
//!   binary swap sends a `deploy-refused` notice (#2590), so the operator is
//!   never blind to an autonomous deploy.
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

/// A bare git commit-ish is 4–64 hex chars. Enforced before shelling `git` so a
/// ref can never be mistaken for a flag or smuggle shell/path metacharacters.
fn is_hex_commitish(s: &str) -> bool {
    let n = s.len();
    (4..=64).contains(&n) && s.bytes().all(|c| c.is_ascii_hexdigit())
}

// ─────────────────────────── injected seams ────────────────────────────────

/// Result of building + verifying the canary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanaryResult {
    pub passed: bool,
    pub detail: String,
    /// Name of the single gate that reddened the canary, when the canary is
    /// RED. Threads per-gate evidence through to the operator refusal so the
    /// root cause is NAMED, not just an `N/M gates` aggregate. `None` on a
    /// green canary.
    pub failing_gate: Option<String>,
    /// Detail of the failing gate (e.g. `tests failed (exit 101): 2 failed`),
    /// paired with [`Self::failing_gate`].
    pub failing_detail: Option<String>,
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
        // Defense-in-depth: only ever hand git bare hex commit-ish. A hex string
        // can never be parsed as an option, closing the argument-injection class
        // even if an upstream caller ever passes attacker-influenced input.
        for arg in [ancestor, descendant] {
            if !is_hex_commitish(arg) {
                return Err(OverseerError::Capability {
                    what: "git.merge-base",
                    detail: format!("refusing non-hex commit-ish argument: {arg:?}"),
                });
            }
        }
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repo_dir)
            // `--` marks end-of-options so a ref can never be read as a flag.
            .args(["merge-base", "--is-ancestor", "--", ancestor, descendant])
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

        // Build + verify the canary first (its result feeds the gate). A HARD
        // error here (as opposed to a red canary, which is `Ok(passed: false)`)
        // is still a deploy ATTEMPT that leaves merged work undeployed, so it
        // must fire the operator notice before surfacing — otherwise the
        // "notify on every attempt" invariant (#2590) would silently break.
        let canary = match self.canary.run_canary(commit) {
            Ok(canary) => canary,
            Err(e) => {
                let notification = OperatorNotification::deploy_refused(
                    commit,
                    &running,
                    &self.repo,
                    &format!("canary run failed: {e}"),
                );
                let _ = self.notifier.notify(&notification);
                return Err(e);
            }
        };

        // A rollback check only matters when the target differs from running.
        // A hard error from the ancestry oracle (e.g. the target commit is not
        // present in the oracle's repo → `git merge-base` exit 128) is likewise
        // a notify-worthy failed attempt, not a silent fail-closed.
        let is_ancestor = if commits_equivalent(&running, commit) {
            false
        } else {
            match self.ancestry.is_ancestor(commit, &running) {
                Ok(is_ancestor) => is_ancestor,
                Err(e) => {
                    let notification = OperatorNotification::deploy_refused(
                        commit,
                        &running,
                        &self.repo,
                        &format!("ancestry check failed: {e}"),
                    );
                    let _ = self.notifier.notify(&notification);
                    return Err(e);
                }
            }
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
            // For a RED canary, NAME the failing gate + detail so the operator
            // refusal evidences the root cause instead of a bare aggregate.
            // Every other refusal (no-op, rollback, crash-loop) keeps its own
            // Display and must NOT be stamped with red-canary gate detail.
            let reason = match &refusal {
                DeployRefusal::RedCanary => match (&canary.failing_gate, &canary.failing_detail) {
                    (Some(gate), Some(detail)) => {
                        format!("{refusal}: gate '{gate}' — {detail} [{}]", canary.detail)
                    }
                    (Some(gate), None) => format!("{refusal}: gate '{gate}' [{}]", canary.detail),
                    _ => format!("{refusal} ({})", canary.detail),
                },
                _ => refusal.to_string(),
            };
            let notification =
                OperatorNotification::deploy_refused(commit, &running, &self.repo, &reason);
            let _ = self.notifier.notify(&notification);
            return Err(OverseerError::Capability {
                what: "deploy_gate",
                detail: reason,
            });
        }

        // Gate passed — announce BEFORE invoking the binary swap. In
        // production the swap restarts/exec-replaces this process and may never
        // return, so a post-swap success notice alone is unreachable on the
        // happy path.
        let starting = OperatorNotification::deploy_starting(
            commit,
            &running,
            &self.repo,
            &format!("canary {} ({})", pass_word(canary.passed), canary.detail),
        );
        let starting_report = self.notifier.notify(&starting);
        debug_assert!(
            starting_report.dispatched(),
            "self-deploy started without a dispatched operator notification"
        );

        // Perform the binary swap. A failed swap is also an operator-visible
        // deploy attempt: notify before surfacing the error.
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

/// Production [`CanaryRunner`]: prepare the canonical self-deploy source at the
/// resolved target commit, build it with the shared self-deploy builder, then
/// run the full relaunch gate sequence (Smoke → UnitTest → GymBaseline →
/// RpcHealth). A target build or gate FAILURE is a RED canary
/// (`passed: false`), NOT a hard error, so the deploy gate refuses it
/// (`RedCanary`) and nothing is swapped. Source-resolution/git failures surface
/// as hard errors to stay fail-safe without falling back to the daemon cwd.
pub struct ProdCanaryRunner {
    source: Box<dyn crate::self_deploy::SelfDeploySourcePreparer>,
    target_canary: Box<dyn TargetCanaryVerifier>,
}

impl ProdCanaryRunner {
    pub fn new() -> Self {
        Self {
            source: Box::new(ExistingRepoSourcePreparer(
                crate::self_deploy::GitSourcePreparer::new(),
            )),
            target_canary: Box::new(SharedTargetCanaryVerifier),
        }
    }

    #[cfg(test)]
    fn with_target_seams(
        source: Box<dyn crate::self_deploy::SelfDeploySourcePreparer>,
        target_canary: Box<dyn TargetCanaryVerifier>,
    ) -> Self {
        Self {
            source,
            target_canary,
        }
    }
}

impl Default for ProdCanaryRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TargetCanaryReport {
    passed: bool,
    passed_gates: usize,
    total_gates: usize,
    /// Name of the gate that reddened the canary, if any (per-gate evidence).
    failing_gate: Option<String>,
    /// Detail of the failing gate, paired with `failing_gate`.
    failing_detail: Option<String>,
    /// Gates skipped because their required endpoint is legitimately absent in
    /// the isolated canary (surfaced for diagnosability; never reds the canary).
    skipped_gates: Vec<String>,
}

trait TargetCanaryVerifier: Send + Sync {
    fn build_and_verify(
        &self,
        source: &dyn crate::self_deploy::SelfDeploySourcePreparer,
        target_commit: &str,
    ) -> Result<TargetCanaryReport, crate::safe_update::SafeUpdateError>;
}

struct SharedTargetCanaryVerifier;

struct ExistingRepoSourcePreparer(crate::self_deploy::GitSourcePreparer);

impl crate::self_deploy::SelfDeploySourcePreparer for ExistingRepoSourcePreparer {
    fn prepare(
        &self,
        target_commit: &str,
    ) -> Result<std::path::PathBuf, crate::safe_update::SafeUpdateError> {
        self.0.prepare_existing_repo(target_commit)
    }
}

impl TargetCanaryVerifier for SharedTargetCanaryVerifier {
    fn build_and_verify(
        &self,
        source: &dyn crate::self_deploy::SelfDeploySourcePreparer,
        target_commit: &str,
    ) -> Result<TargetCanaryReport, crate::safe_update::SafeUpdateError> {
        let results = crate::self_deploy::source_prep::prepare_build_and_verify_canary(
            source,
            target_commit,
            &crate::self_deploy::self_deploy_target_dir(),
        )?;
        let passed = crate::self_relaunch::all_gates_passed(&results);
        let passed_gates = results.iter().filter(|r| r.passed).count();
        // Skips uphold `skipped ⇒ passed`, so isolate a GENUINE failure (a gate
        // that neither passed nor skipped) as the named root cause.
        let failing = results.iter().find(|r| !r.passed && !r.skipped);
        let skipped_gates = results
            .iter()
            .filter(|r| r.skipped)
            .map(|r| r.gate.to_string())
            .collect();
        Ok(TargetCanaryReport {
            passed,
            passed_gates,
            total_gates: results.len(),
            failing_gate: failing.map(|r| r.gate.to_string()),
            failing_detail: failing.map(|r| r.detail.clone()),
            skipped_gates,
        })
    }
}

impl CanaryRunner for ProdCanaryRunner {
    fn run_canary(&self, target_commit: &str) -> Result<CanaryResult, OverseerError> {
        use crate::safe_update::SafeUpdateError;

        match self
            .target_canary
            .build_and_verify(self.source.as_ref(), target_commit)
        {
            Ok(report) => {
                let mut detail = format!("{}/{} gates", report.passed_gates, report.total_gates);
                if !report.skipped_gates.is_empty() {
                    detail.push_str(&format!(" (skipped: {})", report.skipped_gates.join(", ")));
                }
                Ok(CanaryResult {
                    passed: report.passed,
                    detail,
                    failing_gate: report.failing_gate,
                    failing_detail: report.failing_detail,
                })
            }
            Err(SafeUpdateError::BuildFailed { detail }) => Ok(CanaryResult {
                passed: false,
                detail: format!("target canary build failed: {detail}"),
                failing_gate: Some("build".to_string()),
                failing_detail: Some(detail),
            }),
            Err(SafeUpdateError::GateFailed { gate, detail }) => Ok(CanaryResult {
                passed: false,
                detail: format!("target canary gate {gate} failed: {detail}"),
                failing_gate: Some(gate),
                failing_detail: Some(detail),
            }),
            Err(e) => Err(OverseerError::Capability {
                what: "target_canary",
                detail: e.to_string(),
            }),
        }
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
/// orchestrator swap + a git ancestry oracle rooted at the SAME self-deploy
/// checkout the drift observer resolves and fetches + the mandatory dual-channel
/// operator notifier. `running_commit` is the binary's embedded build hash;
/// `recent_restart_churn` is read LIVE at assembly time (the daemon rebuilds the
/// Overseer every tick) so the gate's crash-loop refusal sees current churn.
/// `repo` labels the operator notification.
///
/// **Ancestry repo (issue #2590):** the deploy TARGET commit is resolved and
/// `git fetch`-ed by [`GitDeployDriftObserver`](crate::overseer::deploy_trigger)
/// in the canonical self-deploy checkout (`SIMARD_SELF_DEPLOY_REPO` → persistent
/// `~/.simard` checkout), NOT the daemon's launch `repo_dir`. Rooting the oracle
/// at that same repo (resolved with a cheap filesystem-only probe — the observer,
/// NOT this per-tick constructor, performs the throttled `git fetch`) guarantees
/// the freshly-merged target commit object is present for the rollback
/// (`is_ancestor`) check; a stale launch repo would exit 128 and (now) fire a
/// `deploy-refused` notice rather than silently fail-closing. Falls back to
/// `repo_dir` only when no self-deploy checkout exists yet (e.g. before the first
/// deploy).
pub fn production_guarded_deployer(
    repo_dir: std::path::PathBuf,
    recent_restart_churn: u64,
    repo: String,
) -> GuardedDeployer {
    let ancestry_repo = {
        let preparer = crate::self_deploy::GitSourcePreparer::new();
        // Resolve the canonical checkout with a CHEAP, filesystem-only probe. Do
        // NOT `git fetch` here (#2590 audit): this constructor runs on EVERY
        // overseer tick via `build_overseer`, so an eager network fetch would add
        // blocking, timeout-less I/O to every tick — and a hung fetch would stall
        // the whole OODA loop (the overlap guard then skips subsequent ticks),
        // not just deploy. Freshness of the merged target object is already
        // guaranteed by `GitDeployDriftObserver::observe`, which fetches this SAME
        // repo earlier in the same cycle (throttled) before any deploy is planned;
        // and if the object were still absent the ancestry check surfaces a
        // `deploy-refused` operator notice rather than swapping.
        preparer.resolve_existing_repo().unwrap_or(repo_dir)
    };
    GuardedDeployer::new(
        Box::new(ProdCanaryRunner::new()),
        Box::new(OrchestratedBinaryDeployer),
        Box::new(GitAncestryOracle {
            repo_dir: ancestry_repo,
        }),
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
    use crate::safe_update::SafeUpdateError;
    use std::path::PathBuf;
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
    fn hex_commitish_accepts_valid_and_rejects_injection() {
        assert!(is_hex_commitish("aaaa"));
        assert!(is_hex_commitish("deadBEEF"));
        assert!(is_hex_commitish(&"a".repeat(40)));
        assert!(is_hex_commitish(&"f".repeat(64)));
        // Too short / too long.
        assert!(!is_hex_commitish("abc"));
        assert!(!is_hex_commitish(&"a".repeat(65)));
        // Non-hex / injection shapes.
        assert!(!is_hex_commitish(""));
        assert!(!is_hex_commitish("--help"));
        assert!(!is_hex_commitish("HEAD"));
        assert!(!is_hex_commitish("main"));
        assert!(!is_hex_commitish("dead beef"));
        assert!(!is_hex_commitish("aaaa;rm -rf /"));
        assert!(!is_hex_commitish("../etc/passwd"));
    }

    #[test]
    fn real_ancestry_oracle_refuses_non_hex_without_shelling_git() {
        let oracle = GitAncestryOracle {
            repo_dir: std::path::PathBuf::from("."),
        };
        // A flag-shaped or otherwise non-hex arg is rejected before git runs.
        let err = oracle
            .is_ancestor("--upload-pack=touch pwned", &"a".repeat(40))
            .unwrap_err();
        match err {
            OverseerError::Capability { what, .. } => assert_eq!(what, "git.merge-base"),
            other => panic!("expected Capability error, got {other:?}"),
        }
        assert!(oracle.is_ancestor(&"a".repeat(40), "HEAD").is_err());
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
                failing_gate: None,
                failing_detail: None,
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
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "operator notified before and after deploy");
        assert_eq!(seen[0].kind, "deploy-starting");
        assert_eq!(seen[1].kind, "deploy");
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

    // ── hard-error paths notify (issue #2590 Finding 1b) ──────────────────────
    //
    // A red canary is `Ok(passed: false)` (tested above via `deployer(false,..)`).
    // These cover the distinct HARD-error paths — `run_canary`/`is_ancestor`
    // returning `Err` (e.g. the ancestry oracle's repo lacks the freshly-merged
    // target commit → `git merge-base` exit 128). Both must fire a
    // `deploy-refused` operator notice before surfacing the error, so a genuine
    // deploy attempt can never fail silently.

    struct ErrCanary;
    impl CanaryRunner for ErrCanary {
        fn run_canary(&self, _t: &str) -> Result<CanaryResult, OverseerError> {
            Err(OverseerError::Capability {
                what: "canary",
                detail: "build spawn failed".to_string(),
            })
        }
    }

    struct ErrAncestry;
    impl AncestryOracle for ErrAncestry {
        fn is_ancestor(&self, _a: &str, _d: &str) -> Result<bool, OverseerError> {
            Err(OverseerError::Capability {
                what: "git.merge-base",
                detail: "unexpected exit Some(128)".to_string(),
            })
        }
    }

    #[allow(clippy::type_complexity)]
    fn deployer_with(
        canary: Box<dyn CanaryRunner>,
        ancestry: Box<dyn AncestryOracle>,
    ) -> (
        GuardedDeployer,
        Arc<Mutex<usize>>,
        Arc<Mutex<Vec<OperatorNotification>>>,
    ) {
        let deployed = Arc::new(Mutex::new(0));
        let seen = Arc::new(Mutex::new(vec![]));
        let notifier = DualChannelNotifier::new(vec![Box::new(Capture(seen.clone()))]);
        let gd = GuardedDeployer::new(
            canary,
            Box::new(FakeDeployerShared(deployed.clone())),
            ancestry,
            notifier,
            "aaaaaaaaaaaa".to_string(),
            0,
            "rysweet/Simard".to_string(),
        );
        (gd, deployed, seen)
    }

    #[test]
    fn deploy_notifies_when_canary_run_errors_hard() {
        let (gd, deployed, seen) =
            deployer_with(Box::new(ErrCanary), Box::new(FakeAncestry(false)));
        let err = gd.deploy("bbbbbbbbbbbb").unwrap_err();
        assert!(format!("{err}").contains("canary"));
        assert_eq!(*deployed.lock().unwrap(), 0, "no binary swap on hard error");
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "operator notified on a hard canary error"
        );
        assert_eq!(seen.lock().unwrap()[0].kind, "deploy-refused");
    }

    #[test]
    fn deploy_notifies_when_ancestry_oracle_errors_hard() {
        // Mirrors the cross-repo mismatch: the target commit is absent in the
        // oracle's repo so `git merge-base --is-ancestor` errors. The attempt
        // must NOT fail-close silently — the operator is notified.
        let (gd, deployed, seen) = deployer_with(Box::new(FakeCanary(true)), Box::new(ErrAncestry));
        let err = gd.deploy("bbbbbbbbbbbb").unwrap_err();
        assert!(format!("{err}").contains("merge-base"));
        assert_eq!(*deployed.lock().unwrap(), 0, "no binary swap on hard error");
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "operator notified on a hard ancestry error"
        );
        assert_eq!(seen.lock().unwrap()[0].kind, "deploy-refused");
    }

    // ── target-aware production canary seams (issue #2590 crusty finding) ───

    struct RecordingSource {
        seen_targets: Arc<Mutex<Vec<String>>>,
        mode: SourceMode,
    }

    enum SourceMode {
        Ok,
        ResolveErr,
    }

    impl crate::self_deploy::SelfDeploySourcePreparer for RecordingSource {
        fn prepare(&self, target_commit: &str) -> Result<PathBuf, SafeUpdateError> {
            self.seen_targets
                .lock()
                .unwrap()
                .push(target_commit.to_string());
            match self.mode {
                SourceMode::Ok => Ok(PathBuf::from("/nonexistent/prepared-target")),
                SourceMode::ResolveErr => Err(SafeUpdateError::SourceResolveFailed {
                    detail: "canonical source unavailable".to_string(),
                }),
            }
        }
    }

    struct RecordingTargetCanaryVerifier {
        outcome: VerifierOutcome,
    }

    enum VerifierOutcome {
        Pass,
        BuildFail,
    }

    impl TargetCanaryVerifier for RecordingTargetCanaryVerifier {
        fn build_and_verify(
            &self,
            source: &dyn crate::self_deploy::SelfDeploySourcePreparer,
            target_commit: &str,
        ) -> Result<TargetCanaryReport, SafeUpdateError> {
            let _repo = source.prepare(target_commit)?;
            match self.outcome {
                VerifierOutcome::Pass => Ok(TargetCanaryReport {
                    passed: true,
                    passed_gates: 4,
                    total_gates: 4,
                    failing_gate: None,
                    failing_detail: None,
                    skipped_gates: vec![],
                }),
                VerifierOutcome::BuildFail => Err(SafeUpdateError::BuildFailed {
                    detail: "target build failed".to_string(),
                }),
            }
        }
    }

    fn target_canary(
        mode: SourceMode,
        outcome: VerifierOutcome,
    ) -> (ProdCanaryRunner, Arc<Mutex<Vec<String>>>) {
        let seen_targets = Arc::new(Mutex::new(vec![]));
        let runner = ProdCanaryRunner::with_target_seams(
            Box::new(RecordingSource {
                seen_targets: seen_targets.clone(),
                mode,
            }),
            Box::new(RecordingTargetCanaryVerifier { outcome }),
        );
        (runner, seen_targets)
    }

    #[test]
    fn prod_canary_runner_prepares_and_verifies_the_target_commit() {
        let (runner, seen_targets) = target_canary(SourceMode::Ok, VerifierOutcome::Pass);
        let result = runner.run_canary("bbbbbbbbbbbb").expect("target canary");
        assert!(result.passed);
        assert_eq!(result.detail, "4/4 gates");
        assert_eq!(
            seen_targets.lock().unwrap().as_slice(),
            ["bbbbbbbbbbbb"],
            "production canary must prepare/build the requested target, not cwd"
        );
    }

    #[test]
    fn target_canary_build_failure_is_red_canary_gate_refusal() {
        let (runner, seen_targets) = target_canary(SourceMode::Ok, VerifierOutcome::BuildFail);
        let (gd, deployed, seen) = deployer_with(Box::new(runner), Box::new(FakeAncestry(false)));
        let err = gd.deploy("bbbbbbbbbbbb").unwrap_err();
        assert!(format!("{err}").contains("red canary"));
        assert_eq!(
            seen_targets.lock().unwrap().as_slice(),
            ["bbbbbbbbbbbb"],
            "failed target build must still be for the target commit"
        );
        assert_eq!(*deployed.lock().unwrap(), 0, "red canary blocks swap");
        assert_eq!(seen.lock().unwrap()[0].kind, "deploy-refused");
    }

    #[test]
    fn target_canary_source_resolve_error_is_fail_safe_without_swap() {
        let (runner, seen_targets) = target_canary(SourceMode::ResolveErr, VerifierOutcome::Pass);
        let (gd, deployed, seen) = deployer_with(Box::new(runner), Box::new(FakeAncestry(false)));
        let err = gd.deploy("bbbbbbbbbbbb").unwrap_err();
        assert!(format!("{err}").contains("target_canary"));
        assert_eq!(seen_targets.lock().unwrap().as_slice(), ["bbbbbbbbbbbb"]);
        assert_eq!(*deployed.lock().unwrap(), 0, "resolve error blocks swap");
        assert_eq!(seen.lock().unwrap()[0].kind, "deploy-refused");
    }

    // ── pre-swap notification must precede process replacement ──────────────

    struct EventChannel {
        name: &'static str,
        events: Arc<Mutex<Vec<String>>>,
    }

    impl NotifyChannel for EventChannel {
        fn name(&self) -> &str {
            self.name
        }

        fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
            self.events
                .lock()
                .unwrap()
                .push(format!("notify:{}:{}", self.name, n.kind));
            ChannelDelivery::Sent
        }
    }

    struct EventDeployer(Arc<Mutex<Vec<String>>>);

    impl BinaryDeployer for EventDeployer {
        fn deploy_binary(&self, target: &str) -> Result<String, OverseerError> {
            self.0.lock().unwrap().push("swap".to_string());
            Ok(target.to_string())
        }
    }

    #[test]
    fn deploy_notifies_both_channels_before_swap_on_success_path() {
        let events = Arc::new(Mutex::new(vec![]));
        let notifier = DualChannelNotifier::new(vec![
            Box::new(EventChannel {
                name: "email",
                events: events.clone(),
            }),
            Box::new(EventChannel {
                name: "signal",
                events: events.clone(),
            }),
        ]);
        let gd = GuardedDeployer::new(
            Box::new(FakeCanary(true)),
            Box::new(EventDeployer(events.clone())),
            Box::new(FakeAncestry(false)),
            notifier,
            "aaaaaaaaaaaa".to_string(),
            0,
            "rysweet/Simard".to_string(),
        );

        gd.deploy("bbbbbbbbbbbb").expect("deploy");

        let events = events.lock().unwrap().clone();
        assert_eq!(
            &events[..3],
            [
                "notify:email:deploy-starting",
                "notify:signal:deploy-starting",
                "swap"
            ],
            "starting notification must reach both channels before swap"
        );
        assert!(events.contains(&"notify:email:deploy".to_string()));
        assert!(events.contains(&"notify:signal:deploy".to_string()));
    }

    struct PanicIfSwapped;

    impl BinaryDeployer for PanicIfSwapped {
        fn deploy_binary(&self, _target: &str) -> Result<String, OverseerError> {
            panic!("swap must not run when mandatory pre-swap notification is undispatched")
        }
    }

    #[test]
    #[should_panic(expected = "self-deploy started without a dispatched operator notification")]
    fn deploy_starting_notification_dispatch_is_mandatory() {
        let gd = GuardedDeployer::new(
            Box::new(FakeCanary(true)),
            Box::new(PanicIfSwapped),
            Box::new(FakeAncestry(false)),
            DualChannelNotifier::new(vec![]),
            "aaaaaaaaaaaa".to_string(),
            0,
            "rysweet/Simard".to_string(),
        );
        let _ = gd.deploy("bbbbbbbbbbbb");
    }

    // ── per-gate evidence + skip diagnosability (canary-gate #2590) ──────────
    //
    // TDD (Step 7): these specify the additive per-gate evidence threaded from
    // `TargetCanaryReport` → `CanaryResult` → the operator `deploy-refused`
    // notification, and that a skip stays green and is surfaced in the detail.
    // They FAIL until `deploy.rs` gains `failing_gate` / `failing_detail` on
    // `CanaryResult`, `failing_gate` / `failing_detail` / `skipped_gates` on
    // `TargetCanaryReport`, and the wiring that composes a named red-canary
    // refusal (expected RED state).

    /// A canary that returns a RED result already carrying the named failing
    /// gate + detail, exactly as the production `build_and_verify` now threads
    /// it through. Exercises the operator-refusal composition.
    struct NamedRedCanary {
        gate: &'static str,
        detail: &'static str,
    }
    impl CanaryRunner for NamedRedCanary {
        fn run_canary(&self, _t: &str) -> Result<CanaryResult, OverseerError> {
            Ok(CanaryResult {
                passed: false,
                detail: "3/4 gates".to_string(),
                failing_gate: Some(self.gate.to_string()),
                failing_detail: Some(self.detail.to_string()),
            })
        }
    }

    /// A target-canary verifier that returns a prebuilt report, letting us test
    /// how `ProdCanaryRunner::run_canary` threads per-gate evidence + skips into
    /// the `CanaryResult` without a real build.
    struct ReportVerifier(TargetCanaryReport);
    impl TargetCanaryVerifier for ReportVerifier {
        fn build_and_verify(
            &self,
            source: &dyn crate::self_deploy::SelfDeploySourcePreparer,
            target_commit: &str,
        ) -> Result<TargetCanaryReport, SafeUpdateError> {
            let _ = source.prepare(target_commit)?;
            Ok(self.0.clone())
        }
    }

    /// A verifier whose `build_and_verify` fails a NAMED gate (hard
    /// `GateFailed`). The `run_canary` `GateFailed` arm must carry that gate
    /// name downstream, not just a generic detail string.
    struct GateFailVerifier;
    impl TargetCanaryVerifier for GateFailVerifier {
        fn build_and_verify(
            &self,
            source: &dyn crate::self_deploy::SelfDeploySourcePreparer,
            target_commit: &str,
        ) -> Result<TargetCanaryReport, SafeUpdateError> {
            let _ = source.prepare(target_commit)?;
            Err(SafeUpdateError::GateFailed {
                gate: "rpc-health".to_string(),
                detail: "rpc responded but reported degraded health".to_string(),
            })
        }
    }

    fn prod_runner_with(verifier: Box<dyn TargetCanaryVerifier>) -> ProdCanaryRunner {
        ProdCanaryRunner::with_target_seams(
            Box::new(RecordingSource {
                seen_targets: Arc::new(Mutex::new(vec![])),
                mode: SourceMode::Ok,
            }),
            verifier,
        )
    }

    #[test]
    fn run_canary_threads_named_failing_gate_from_red_report() {
        let report = TargetCanaryReport {
            passed: false,
            passed_gates: 3,
            total_gates: 4,
            failing_gate: Some("unit-test".to_string()),
            failing_detail: Some("tests failed (exit 101): 2 failed".to_string()),
            skipped_gates: vec![],
        };
        let runner = prod_runner_with(Box::new(ReportVerifier(report)));
        let result = runner.run_canary("bbbbbbbbbbbb").expect("canary");
        assert!(!result.passed);
        assert_eq!(result.failing_gate.as_deref(), Some("unit-test"));
        assert!(
            result
                .failing_detail
                .as_deref()
                .unwrap_or_default()
                .contains("2 failed"),
            "failing_detail must survive into the CanaryResult: {:?}",
            result.failing_detail
        );
    }

    #[test]
    fn run_canary_appends_skipped_gates_to_detail_and_stays_green() {
        let report = TargetCanaryReport {
            passed: true,
            passed_gates: 3,
            total_gates: 4,
            failing_gate: None,
            failing_detail: None,
            skipped_gates: vec!["rpc-health".to_string()],
        };
        let runner = prod_runner_with(Box::new(ReportVerifier(report)));
        let result = runner.run_canary("bbbbbbbbbbbb").expect("canary");
        assert!(result.passed, "a skip must not red the canary");
        assert!(result.failing_gate.is_none());
        assert!(
            result.detail.contains("skipped: rpc-health"),
            "detail must surface the skipped gate, got: {}",
            result.detail
        );
    }

    #[test]
    fn run_canary_green_report_has_no_skip_suffix() {
        let report = TargetCanaryReport {
            passed: true,
            passed_gates: 4,
            total_gates: 4,
            failing_gate: None,
            failing_detail: None,
            skipped_gates: vec![],
        };
        let runner = prod_runner_with(Box::new(ReportVerifier(report)));
        let result = runner.run_canary("bbbbbbbbbbbb").expect("canary");
        assert!(result.passed);
        assert_eq!(result.detail, "4/4 gates");
        assert!(!result.detail.contains("skipped"), "{}", result.detail);
        assert!(result.failing_gate.is_none());
    }

    #[test]
    fn run_canary_gate_failed_hard_error_names_the_gate() {
        // A `GateFailed` is a RED canary (`Ok(passed:false)`), not a hard error,
        // and must carry the named gate downstream.
        let runner = prod_runner_with(Box::new(GateFailVerifier));
        let result = runner
            .run_canary("bbbbbbbbbbbb")
            .expect("a gate failure is a red canary, not a hard error");
        assert!(!result.passed);
        assert_eq!(
            result.failing_gate.as_deref(),
            Some("rpc-health"),
            "the GateFailed arm must carry the named gate downstream"
        );
    }

    #[test]
    fn deploy_refused_names_failing_gate_for_red_canary() {
        let deployed = Arc::new(Mutex::new(0));
        let seen = Arc::new(Mutex::new(vec![]));
        let notifier = DualChannelNotifier::new(vec![Box::new(Capture(seen.clone()))]);
        let gd = GuardedDeployer::new(
            Box::new(NamedRedCanary {
                gate: "unit-test",
                detail: "tests failed (exit 101): 2 failed",
            }),
            Box::new(FakeDeployerShared(deployed.clone())),
            Box::new(FakeAncestry(false)),
            notifier,
            "aaaaaaaaaaaa".to_string(),
            0,
            "rysweet/Simard".to_string(),
        );
        let err = gd.deploy("bbbbbbbbbbbb").unwrap_err();
        assert!(format!("{err}").contains("red canary"));
        assert_eq!(*deployed.lock().unwrap(), 0, "red canary blocks swap");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].kind, "deploy-refused");
        // The operator refusal must NAME the failing gate + detail — not just a
        // bare aggregate — so the root cause is evidenced.
        assert!(
            seen[0].problem.contains("unit-test"),
            "refusal must name the failing gate: {}",
            seen[0].problem
        );
        assert!(
            seen[0].problem.contains("2 failed"),
            "refusal must carry the failing detail: {}",
            seen[0].problem
        );
    }

    #[test]
    fn deploy_refused_for_non_red_refusal_is_not_mislabeled_as_red_canary() {
        // A NoOp refusal (target == running) must keep its own Display and must
        // NOT be stamped with red-canary named-gate detail, even though the
        // canary result type now carries failing_gate fields.
        let (gd, deployed, seen) = deployer(true, false, 0);
        let err = gd.deploy("aaaaaaaaaaaa").unwrap_err(); // target == running → NoOp
        assert!(format!("{err}").contains("no-op"));
        assert_eq!(*deployed.lock().unwrap(), 0);
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].kind, "deploy-refused");
        assert!(seen[0].problem.contains("no-op"), "{}", seen[0].problem);
        assert!(
            !seen[0].problem.contains("red canary"),
            "a no-op refusal must not be mislabeled a red canary: {}",
            seen[0].problem
        );
    }
}
