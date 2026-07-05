//! Daemon wiring for the acting Overseer co-process (M2+).
//!
//! This module is the seam that turns the Overseer from an inert design sketch
//! into a live, in-process co-process the OODA daemon drives on a cadence. It
//! provides four things:
//!
//! 1. [`OverseerCadence`] — a tiny, injected-clock-testable interval scheduler
//!    (mirrors the daemon's sibling `should_run_*` periodic-task pattern).
//! 2. [`overseer_tick`] / [`run_overseer_tick_isolated`] — the drive helper that
//!    runs one meta-OODA turn (`run_cycle` → act on every admitted intervention)
//!    with structured per-tick tracing and **panic isolation**, so an error or
//!    panic in a tick is caught and logged and never crashes or stalls the
//!    daemon or the OODA loop.
//! 3. [`assemble_capabilities`] / [`build_overseer`] — construct the production
//!    [`Overseer`] from the already-shipped capability adapters, under a
//!    DISTINCT anti-recursion identity ([`overseer_identity`]) and with acting
//!    autonomy (verify-merge + HIGH-RISK) enabled.
//! 4. [`BoardGoalCurator`] — the production [`GoalCurator`] adapter that reads
//!    Simard's live goal board so Orient dedups against in-flight engineer work
//!    and the Overseer never fights the OODA loop.
//!
//! Reuse (never reimplementation): every capability is a thin adapter over an
//! existing Simard entry point — `SnapshotStatusReader` (status::assemble),
//! `SmartOrchestratorLauncher` (amplihack recipe run), `MergePrOps` (gated merge
//! authority: poll-until-green, NEVER `--admin`/`--no-verify`, notify-on-merge),
//! `MeetingGoalTransfer`, `StewardshipIssueFiler`, `SelfQualityAuditor`, and the
//! goal board via `goal_curation`.

use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::goal_curation::{BacklogItem, GoalBoard, GoalProgress};
use crate::goal_curation::{
    DEFAULT_STEWARD_SCORE, add_backlog_item, load_goal_board, save_goal_board,
};

use crate::overseer::audit::SelfQualityAuditor;
use crate::overseer::capabilities::{
    BlockedGoal, DeployReport, Deployer, GoalBrief, GoalCurator, InFlightItem, OverseerError,
};
use crate::overseer::config::{
    goal_health_enabled, overseer_author_login, overseer_interval_secs, whisper_enabled,
};
use crate::overseer::deploy::GuardedDeployer;
use crate::overseer::guardrails::RecursionGuard;
use crate::overseer::launch::SmartOrchestratorLauncher;
use crate::overseer::meeting_ops::MeetingGoalTransfer;
use crate::overseer::merge_ops::MergePrOps;
use crate::overseer::notify::DualChannelNotifier;
use crate::overseer::observer::StewardshipIssueFiler;
use crate::overseer::sensor::{
    SnapshotStatusReader, blocked_goals_from_board, in_flight_from_board,
};
use crate::overseer::{ActOutcome, Capabilities, Overseer};

// ─────────────────────────── cadence scheduler ─────────────────────────────

/// Interval scheduler for the periodic Overseer tick. Deliberately clock-free:
/// the caller feeds a monotonic `now_secs`, so production uses
/// `Instant::elapsed().as_secs()` while tests inject a virtual clock and assert
/// the tick fires exactly on/after the interval boundary. Mirrors the daemon's
/// sibling periodic tasks (backup / disk-health / brain-introspection).
#[derive(Clone, Copy, Debug)]
pub struct OverseerCadence {
    interval_secs: u64,
    last_tick_secs: u64,
}

impl OverseerCadence {
    /// Start the cadence at `now_secs` (the first tick fires one full interval
    /// later — nothing useful to do at t=0). The interval is floored at 1s so a
    /// pathological `0` can never busy-fire.
    pub fn new(interval_secs: u64, now_secs: u64) -> Self {
        Self {
            interval_secs: interval_secs.max(1),
            last_tick_secs: now_secs,
        }
    }

    /// The interval this cadence fires on (seconds).
    pub fn interval_secs(&self) -> u64 {
        self.interval_secs
    }

    /// Return `true` (and advance the last-tick marker to `now_secs`) exactly
    /// when at least one interval has elapsed since the previous tick; otherwise
    /// `false` and no state change. Monotonic: `now_secs` going backwards never
    /// fires.
    pub fn due(&mut self, now_secs: u64) -> bool {
        if now_secs.saturating_sub(self.last_tick_secs) >= self.interval_secs {
            self.last_tick_secs = now_secs;
            true
        } else {
            false
        }
    }
}

// ─────────────────────────── per-tick report ───────────────────────────────

/// Structured tally of one Overseer tick. Every field is emitted as a
/// `tracing` key so a tick is fully observable without `println!`/`eprintln!`.
///
/// Derives `Serialize`/`Deserialize` (additive; no logic change) so the acting
/// tick's outcome can be recorded verbatim into the durable
/// [activity feed](crate::overseer::activity).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverseerTickReport {
    /// Problems raised this cycle (post-dedup against in-flight work).
    pub problems: usize,
    /// Deduplicated issues filed via the stewardship path.
    pub issues_filed: usize,
    /// Fix workstreams launched (smart-orchestrator/default-workflow).
    pub recipes_launched: usize,
    /// Green, merge-ready PRs verified-and-merged (normal merge; never admin).
    pub prs_merged: usize,
    /// Guarded deploys performed (through the canary/self-deploy gates).
    pub deploys: usize,
    /// Interventions handed off to the operator (escalations).
    pub escalations: usize,
    /// Interventions the gates held (autonomy/budget/conflict).
    pub held: usize,
    /// Advisory whispers delivered into Simard's OODA inbox this tick.
    pub whispers: usize,
    /// Whispers suppressed by the dedup window / per-hour cap this tick.
    pub whispers_suppressed: usize,
    /// False-parked standing/perpetual goals auto-unblocked + reactivated this
    /// tick (the self-heal path).
    pub goals_unblocked: usize,
    /// Genuinely-blocked "needs human review" goals escalated to the operator
    /// (email + Signal) this tick.
    pub goals_escalated: usize,
    /// Goal-board self-heal / escalation actions suppressed by the dedup gate
    /// this tick.
    pub goals_health_suppressed: usize,
    /// Capability errors encountered while acting (isolated, never fatal).
    pub errors: usize,
    /// Set when the tick itself panicked and was isolated by
    /// [`run_overseer_tick_isolated`].
    pub panicked: bool,
    /// Wall-clock duration of the tick in milliseconds.
    pub duration_ms: u64,
}

/// Run ONE Overseer meta-OODA turn: `run_cycle` (Observe→Orient→Decide→plan)
/// then execute every ADMITTED intervention via `act`, tallying outcomes and
/// emitting one structured `tracing` event. Errors from `run_cycle` or any
/// `act` are caught and counted (never propagated) so a single bad capability
/// call cannot abort the tick — and the daemon never sees a `Result::Err`.
///
/// This does NOT catch panics; wrap it in [`run_overseer_tick_isolated`] for
/// the full fail-safe boundary the daemon uses.
pub fn overseer_tick(overseer: &mut Overseer) -> OverseerTickReport {
    let start = Instant::now();
    let mut report = OverseerTickReport::default();

    match overseer.run_cycle() {
        Ok(cycle) => {
            report.problems = cycle.problems.len();
            for planned in &cycle.plan {
                if !planned.admitted {
                    report.held += 1;
                    continue;
                }
                match overseer.act(&planned.intervention) {
                    Ok(outcome) => tally_outcome(&mut report, &outcome),
                    Err(e) => {
                        report.errors += 1;
                        tracing::warn!(
                            target: "overseer::tick",
                            intervention = planned.intervention.label(),
                            error = %e,
                            "overseer intervention failed — isolated, continuing"
                        );
                    }
                }
            }
        }
        Err(e) => {
            report.errors += 1;
            tracing::warn!(
                target: "overseer::tick",
                error = %e,
                "overseer run_cycle failed — isolated, no actions taken"
            );
        }
    }

    report.duration_ms = start.elapsed().as_millis() as u64;
    tracing::info!(
        target: "overseer::tick",
        enabled = true,
        problems = report.problems,
        issues_filed = report.issues_filed,
        recipes_launched = report.recipes_launched,
        prs_merged = report.prs_merged,
        deploys = report.deploys,
        escalations = report.escalations,
        held = report.held,
        whispers = report.whispers,
        whispers_suppressed = report.whispers_suppressed,
        goals_unblocked = report.goals_unblocked,
        goals_escalated = report.goals_escalated,
        goals_health_suppressed = report.goals_health_suppressed,
        errors = report.errors,
        duration_ms = report.duration_ms,
        "overseer tick complete"
    );
    report
}

/// Panic-isolated wrapper around [`overseer_tick`]. A panic inside a capability
/// adapter is caught, logged, and turned into a report with `panicked = true`;
/// the daemon loop continues unaffected. This is the boundary that guarantees
/// "a panicking overseer tick never crashes or stalls the daemon".
pub fn run_overseer_tick_isolated(overseer: &mut Overseer) -> OverseerTickReport {
    let start = Instant::now();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| overseer_tick(overseer))) {
        Ok(report) => report,
        Err(_) => {
            tracing::error!(
                target: "overseer::tick",
                panicked = true,
                "overseer tick panicked — isolated; daemon continues"
            );
            OverseerTickReport {
                panicked: true,
                errors: 1,
                duration_ms: start.elapsed().as_millis() as u64,
                ..OverseerTickReport::default()
            }
        }
    }
}

fn tally_outcome(report: &mut OverseerTickReport, outcome: &ActOutcome) {
    match outcome {
        ActOutcome::Launched(_) => report.recipes_launched += 1,
        ActOutcome::Merged => report.prs_merged += 1,
        ActOutcome::Deployed(_) => report.deploys += 1,
        ActOutcome::IssueFiled(_) => report.issues_filed += 1,
        ActOutcome::Escalated => report.escalations += 1,
        ActOutcome::Whispered { .. } => report.whispers += 1,
        ActOutcome::WhisperSuppressed { .. } => report.whispers_suppressed += 1,
        ActOutcome::GoalUnblocked { .. } => report.goals_unblocked += 1,
        ActOutcome::GoalEscalated { .. } => report.goals_escalated += 1,
        ActOutcome::GoalHealthSuppressed { .. } => report.goals_health_suppressed += 1,
        ActOutcome::ConflictResolved
        | ActOutcome::GoalTransferred
        | ActOutcome::Reported
        | ActOutcome::Audited => {}
    }
}

// ─────────────────────────── identity ──────────────────────────────────────

/// The Overseer's DISTINCT anti-recursion identity: a stable, well-known bot
/// login plus the branch/goal namespaces its launched work uses. Sourced from
/// [`overseer_author_login`] so the daemon, the merge path, and goal dedup all
/// agree on ONE identity that is never the human operator's login. A fully
/// populated guard fails CLOSED and refuses the Overseer's own PRs/commits,
/// branches, and goals.
pub fn overseer_identity() -> RecursionGuard {
    RecursionGuard {
        author_login: overseer_author_login(),
        branch_prefix: "overseer/".to_string(),
        goal_source_tag: "overseer:".to_string(),
    }
}

// ─────────────────────────── goal-board curator ────────────────────────────

/// Production [`GoalCurator`]: reads Simard's live goal board so Orient dedups
/// the Overseer's problems against in-flight engineer goals, and enqueues
/// proposed goals onto the backlog under the Overseer's own source tag.
///
/// Reuse: `goal_curation::{load_goal_board, add_backlog_item, save_goal_board}`
/// and [`in_flight_from_board`].
pub struct BoardGoalCurator {
    mem: Arc<dyn CognitiveMemoryOps>,
}

impl BoardGoalCurator {
    pub fn new(mem: Arc<dyn CognitiveMemoryOps>) -> Self {
        Self { mem }
    }

    fn load(&self) -> Result<GoalBoard, OverseerError> {
        load_goal_board(self.mem.as_ref()).map_err(|e| OverseerError::Capability {
            what: "goal_board.load",
            detail: e.to_string(),
        })
    }
}

impl GoalCurator for BoardGoalCurator {
    fn propose(&self, goal: &GoalBrief) -> Result<(), OverseerError> {
        let mut board = self.load()?;
        let id = format!("overseer-{}", slugify(&goal.title));
        // Idempotent: never double-enqueue nor collide with an active goal.
        if board.backlog.iter().any(|b| b.id == id) || board.active.iter().any(|g| g.id == id) {
            return Ok(());
        }
        let item = BacklogItem {
            id,
            description: format!("{} — {}", goal.title, goal.rationale),
            // Stamp the Overseer's DISTINCT source tag so anti-recursion never
            // re-opens a goal the Overseer itself filed.
            source: format!("overseer:{}", goal.target_repo),
            score: DEFAULT_STEWARD_SCORE,
        };
        add_backlog_item(&mut board, item).map_err(|e| OverseerError::Capability {
            what: "goal_board.enqueue",
            detail: e.to_string(),
        })?;
        save_goal_board(&board, self.mem.as_ref()).map_err(|e| OverseerError::Capability {
            what: "goal_board.save",
            detail: e.to_string(),
        })
    }

    fn in_flight(&self) -> Result<Vec<InFlightItem>, OverseerError> {
        Ok(in_flight_from_board(&self.load()?))
    }

    fn blocked_goals(&self) -> Result<Vec<BlockedGoal>, OverseerError> {
        Ok(blocked_goals_from_board(&self.load()?))
    }

    fn unblock(&self, goal_id: &str) -> Result<(), OverseerError> {
        // The exact `simard goal unblock` mutation: restore the blocked goal to
        // `NotStarted` so the next OODA cycle re-enters the spawn path. Reuses
        // the shipped `load_goal_board` / `save_goal_board` under the flock
        // write-lock (`save_goal_board` acquires `BoardWriteLock`).
        let mut board = self.load()?;
        let goal = board
            .active
            .iter_mut()
            .find(|g| g.id == goal_id)
            .ok_or_else(|| OverseerError::Capability {
                what: "goal_board.unblock",
                detail: format!("goal '{goal_id}' not found on the active board"),
            })?;
        goal.status = GoalProgress::NotStarted;
        save_goal_board(&board, self.mem.as_ref()).map_err(|e| OverseerError::Capability {
            what: "goal_board.save",
            detail: e.to_string(),
        })
    }
}

/// Lowercase, hyphenate, and bound a title into a stable backlog id fragment.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(48));
    let mut last_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 48 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

// ─────────────────────────── deployer (safe stub) ──────────────────────────

/// A [`Deployer`] that REFUSES autonomous deploys. The deterministic Decide
/// mapping the wired daemon uses never emits `Intervention::Deploy`, so a live
/// deploy is never planned through this path; if one ever were, it escalates
/// rather than blindly swapping the binary. Guarded self-deploy stays the
/// operator's canary/self-deploy gates (unit-tested via `GuardedDeployer` and
/// `evaluate_deploy_gate`), never a blind deploy from the acting loop.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefuseDeployer;

impl Deployer for RefuseDeployer {
    fn deploy(&self, _commit: &str) -> Result<DeployReport, OverseerError> {
        Err(OverseerError::Capability {
            what: "deploy",
            detail: "autonomous deploy is not wired into the acting loop — escalate to the \
                     operator's canary/self-deploy gates (never a blind deploy)"
                .to_string(),
        })
    }

    fn deployed_commit(&self) -> Result<String, OverseerError> {
        Ok(GuardedDeployer::running_commit_marker().to_string())
    }
}

// ─────────────────────────── assembly ──────────────────────────────────────

/// Assemble the production [`Capabilities`] from the already-shipped adapters.
/// Every handle is a thin reuse of an existing Simard subsystem; the merge path
/// carries the Overseer's DISTINCT anti-recursion identity so it can never merge
/// its OWN PR.
pub fn assemble_capabilities(
    mem: Arc<dyn CognitiveMemoryOps>,
    repo_root: std::path::PathBuf,
    state_root: std::path::PathBuf,
) -> Capabilities {
    Capabilities {
        status: Box::new(SnapshotStatusReader::from_env()),
        recipes: Box::new(SmartOrchestratorLauncher::from_env()),
        prs: Box::new(MergePrOps::from_env().with_recursion_guard(overseer_identity())),
        deployer: Box::new(RefuseDeployer),
        meetings: Box::new(MeetingGoalTransfer::from_env()),
        issues: Box::new(StewardshipIssueFiler::new(Arc::new(
            crate::stewardship::RealGhClient,
        ))),
        goals: Box::new(BoardGoalCurator::new(mem)),
        auditor: Box::new(SelfQualityAuditor::from_env(repo_root, state_root)),
    }
}

/// Build the production acting [`Overseer`]: the assembled capabilities, the
/// DISTINCT anti-recursion identity, and acting autonomy ON (verify-merge +
/// HIGH-RISK). The merge path still only merges GREEN, merge-ready PRs through
/// the gated authority (never `--admin`/`--no-verify`) and notifies the operator
/// on every merge; the review gate is fail-closed until an operator wires a
/// reviewer, so nothing is merged unreviewed.
pub fn build_overseer(
    mem: Arc<dyn CognitiveMemoryOps>,
    repo_root: std::path::PathBuf,
    state_root: std::path::PathBuf,
) -> Overseer {
    Overseer::new(assemble_capabilities(mem, repo_root, state_root))
        .with_verify_merge_autonomy(true)
        .with_high_risk_autonomy(true)
        .with_identity(overseer_identity())
        // The Simard Whisperer: advisory steering notes onto the SAME
        // meeting-handoff inbox the OODA observe step scans. Enabled by default
        // (opt-out via SIMARD_OVERSEER_WHISPER), consistent with the acting
        // Overseer's opt-out gate.
        .with_whisper_enabled(whisper_enabled())
        .with_whisper_sink(Box::new(
            crate::overseer::whisper_ops::MeetingHandoffWhisperSink::new(
                crate::meeting_facilitator::default_handoff_dir(),
            ),
        ))
        // Goal-board health: self-heal false-parked perpetual goals and escalate
        // genuine "needs human review" blocks to the operator on BOTH channels
        // (email + Signal) via the SAME mandatory notifier the merge path uses.
        // Enabled by default (opt-out via SIMARD_OVERSEER_GOAL_HEALTH).
        .with_goal_health_enabled(goal_health_enabled())
        .with_operator_notifier(Box::new(DualChannelNotifier::from_env()))
}

/// Resolve the acting Overseer's tick cadence (seconds), clamped to the config
/// floor so self-tuning can never drive a hot loop.
pub fn overseer_tick_interval_secs() -> u64 {
    overseer_interval_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overseer::capabilities::{
        AuditReport, AuditScope, Auditor, CheckItem, IssueOutcome, ObservedState,
        OrchestratorRunBrief, PrOps, RecipeBrief, RecipeLauncher, StatusReader, VerifyReport,
        WorkstreamHandle, WorkstreamStatus,
    };
    use crate::overseer::intervention::Intervention;
    use std::sync::Mutex;

    // ── cadence ───────────────────────────────────────────────────────────

    #[test]
    fn cadence_fires_only_after_the_interval_elapses() {
        // Injected virtual clock (seconds). Interval 900s, started at t=0.
        let mut cadence = OverseerCadence::new(900, 0);
        assert!(!cadence.due(1), "1s in — far below interval, no tick");
        assert!(!cadence.due(899), "just under the interval — no tick");
        assert!(cadence.due(900), "exactly at the interval — tick fires");
        assert!(!cadence.due(901), "immediately after a tick — no re-fire");
        assert!(!cadence.due(1799), "still within the next window — no tick");
        assert!(cadence.due(1800), "one more interval elapsed — tick fires");
    }

    #[test]
    fn cadence_interval_is_floored_to_avoid_a_hot_loop() {
        let mut cadence = OverseerCadence::new(0, 0);
        assert_eq!(cadence.interval_secs(), 1);
        assert!(!cadence.due(0));
        assert!(cadence.due(1));
    }

    #[test]
    fn cadence_never_fires_when_the_clock_goes_backwards() {
        let mut cadence = OverseerCadence::new(60, 100);
        assert!(!cadence.due(50), "clock moved backwards — never fires");
        assert!(cadence.due(160), "forward past the interval — fires");
    }

    // ── fakes for the tick driver ─────────────────────────────────────────

    struct FakeStatus(ObservedState);
    impl StatusReader for FakeStatus {
        fn snapshot(&self) -> Result<ObservedState, OverseerError> {
            Ok(self.0.clone())
        }
    }

    /// A status reader that panics — used to prove panic isolation.
    struct PanicStatus;
    impl StatusReader for PanicStatus {
        fn snapshot(&self) -> Result<ObservedState, OverseerError> {
            panic!("boom: capability adapter blew up mid-observe");
        }
    }

    struct FakeRecipes;
    impl RecipeLauncher for FakeRecipes {
        fn launch(&self, _b: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
            Ok(WorkstreamHandle {
                id: "ws-1".to_string(),
            })
        }
        fn poll(&self, _h: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError> {
            Ok(WorkstreamStatus::Running)
        }
    }

    type MergeLog = Arc<Mutex<Vec<(String, u32)>>>;

    /// Records every `merge` call and returns `ready` from `verify`, so a tick
    /// that drives a green PR to merge is observable without any network. The
    /// merge log is a shared handle so the test can inspect it after the
    /// recorder is boxed into the Overseer — no `unsafe`.
    struct RecordingPrs {
        ready: bool,
        merges: MergeLog,
    }
    impl RecordingPrs {
        fn new(ready: bool) -> (Self, MergeLog) {
            let merges: MergeLog = Arc::new(Mutex::new(vec![]));
            (
                Self {
                    ready,
                    merges: Arc::clone(&merges),
                },
                merges,
            )
        }
    }
    impl PrOps for RecordingPrs {
        fn verify(&self, _repo: &str, _pr: u32) -> Result<VerifyReport, OverseerError> {
            Ok(VerifyReport {
                ready: self.ready,
                checks: vec![CheckItem {
                    name: "objective gates".to_string(),
                    passed: self.ready,
                    note: "test".to_string(),
                }],
            })
        }
        fn merge(&self, repo: &str, pr: u32) -> Result<(), OverseerError> {
            self.merges.lock().unwrap().push((repo.to_string(), pr));
            Ok(())
        }
        fn resolve_conflict(&self, _repo: &str, _pr: u32) -> Result<(), OverseerError> {
            Ok(())
        }
    }

    struct FakeDeployer;
    impl Deployer for FakeDeployer {
        fn deploy(&self, commit: &str) -> Result<DeployReport, OverseerError> {
            Ok(DeployReport {
                deployed_commit: commit.to_string(),
                gates_passed: true,
            })
        }
        fn deployed_commit(&self) -> Result<String, OverseerError> {
            Ok("deadbeef".to_string())
        }
    }

    struct FakeMeetings;
    impl crate::overseer::capabilities::MeetingHost for FakeMeetings {
        fn transfer_goal(&self, _g: &GoalBrief) -> Result<(), OverseerError> {
            Ok(())
        }
    }

    struct FakeIssues;
    impl crate::overseer::capabilities::IssueFiler for FakeIssues {
        fn file(&self, _run: &OrchestratorRunBrief) -> Result<IssueOutcome, OverseerError> {
            Ok(IssueOutcome::FiledNew {
                url: "https://example/issues/1".to_string(),
            })
        }
    }

    struct FakeGoals(Vec<InFlightItem>);
    impl GoalCurator for FakeGoals {
        fn propose(&self, _g: &GoalBrief) -> Result<(), OverseerError> {
            Ok(())
        }
        fn in_flight(&self) -> Result<Vec<InFlightItem>, OverseerError> {
            Ok(self.0.clone())
        }
    }

    struct FakeAuditor;
    impl Auditor for FakeAuditor {
        fn run_audit(&self, scope: &AuditScope) -> Result<AuditReport, OverseerError> {
            Ok(AuditReport {
                scope: scope.clone(),
                passed: true,
                findings: vec![],
            })
        }
    }

    fn caps_with(status: Box<dyn StatusReader>, prs: Box<dyn PrOps>) -> Capabilities {
        Capabilities {
            status,
            recipes: Box::new(FakeRecipes),
            prs,
            deployer: Box::new(FakeDeployer),
            meetings: Box::new(FakeMeetings),
            issues: Box::new(FakeIssues),
            goals: Box::new(FakeGoals(vec![])),
            auditor: Box::new(FakeAuditor),
        }
    }

    // ── tick driver ───────────────────────────────────────────────────────

    #[test]
    fn tick_drives_run_cycle_then_launches_a_fix_for_a_process_health_signal() {
        // A high distill-failure rate raises one ProcessHealth problem → Decide
        // maps it to a LaunchRecipe → the tick executes it via `act`.
        let observed = ObservedState {
            distill_fail_pct: Some(62.0),
            ..ObservedState::default()
        };
        let (prs, _merges) = RecordingPrs::new(true);
        let mut overseer = Overseer::new(caps_with(Box::new(FakeStatus(observed)), Box::new(prs)));
        let report = overseer_tick(&mut overseer);
        assert_eq!(report.problems, 1);
        assert_eq!(
            report.recipes_launched, 1,
            "the tick must ACT, not just plan"
        );
        assert_eq!(report.errors, 0);
        assert!(!report.panicked);
    }

    #[test]
    fn tick_verifies_and_merges_a_green_ready_pr_and_records_the_merge() {
        // Seed a merge-ready PR signal → Decide maps to VerifyAndMergePr →
        // with verify-merge autonomy ON, the tick merges it (normal merge).
        let observed = ObservedState {
            ready_prs: vec![crate::overseer::capabilities::PrRef {
                repo: "rysweet/Simard".to_string(),
                pr: 42,
            }],
            ..ObservedState::default()
        };
        let (prs, merges) = RecordingPrs::new(true);
        let mut overseer = Overseer::new(caps_with(Box::new(FakeStatus(observed)), Box::new(prs)))
            .with_verify_merge_autonomy(true);
        let report = overseer_tick(&mut overseer);
        assert_eq!(report.prs_merged, 1, "green ready PR must be merged");
        assert_eq!(
            *merges.lock().unwrap(),
            vec![("rysweet/Simard".to_string(), 42)]
        );
    }

    #[test]
    fn tick_holds_the_merge_when_autonomy_is_not_opted_in() {
        let observed = ObservedState {
            ready_prs: vec![crate::overseer::capabilities::PrRef {
                repo: "rysweet/Simard".to_string(),
                pr: 7,
            }],
            ..ObservedState::default()
        };
        // Default autonomy (verify-merge OFF) → the merge is HELD (escalated).
        let (prs, merges) = RecordingPrs::new(true);
        let mut overseer = Overseer::new(caps_with(Box::new(FakeStatus(observed)), Box::new(prs)));
        let report = overseer_tick(&mut overseer);
        assert_eq!(report.prs_merged, 0);
        assert_eq!(report.held, 1, "verify-merge is opt-in; held by default");
        assert!(
            merges.lock().unwrap().is_empty(),
            "nothing merged when held"
        );
    }

    // ── panic isolation ───────────────────────────────────────────────────

    #[test]
    fn a_panicking_tick_is_isolated_and_the_overseer_survives() {
        let (prs, _merges) = RecordingPrs::new(true);
        let mut overseer = Overseer::new(caps_with(Box::new(PanicStatus), Box::new(prs)));
        // The panic inside `snapshot` must be caught, not propagated.
        let report = run_overseer_tick_isolated(&mut overseer);
        assert!(report.panicked, "the tick panicked and was isolated");
        assert_eq!(report.prs_merged, 0);
        // The overseer is still usable — a second isolated tick also survives.
        let report2 = run_overseer_tick_isolated(&mut overseer);
        assert!(report2.panicked);
    }

    #[test]
    fn a_run_cycle_error_is_isolated_without_panicking() {
        struct ErrStatus;
        impl StatusReader for ErrStatus {
            fn snapshot(&self) -> Result<ObservedState, OverseerError> {
                Err(OverseerError::Capability {
                    what: "status",
                    detail: "degraded".to_string(),
                })
            }
        }
        let (prs, _merges) = RecordingPrs::new(true);
        let mut overseer = Overseer::new(caps_with(Box::new(ErrStatus), Box::new(prs)));
        let report = overseer_tick(&mut overseer);
        assert!(!report.panicked, "an error is not a panic");
        assert_eq!(report.errors, 1);
        assert_eq!(report.problems, 0);
    }

    // ── identity ──────────────────────────────────────────────────────────

    #[test]
    fn overseer_identity_is_configured_and_refuses_its_own_pr() {
        let guard = overseer_identity();
        assert!(guard.is_configured(), "identity must be fully populated");
        let own = crate::overseer::guardrails::Subject::Pr {
            repo: "rysweet/Simard".to_string(),
            pr: 1,
            author: guard.author_login.clone(),
        };
        assert!(guard.admit(&own).is_err(), "must refuse its OWN PR");
        let foreign = crate::overseer::guardrails::Subject::Pr {
            repo: "rysweet/Simard".to_string(),
            pr: 2,
            author: "a-human-operator".to_string(),
        };
        assert!(guard.admit(&foreign).is_ok(), "must admit a human's PR");
    }

    #[test]
    fn slugify_is_stable_and_bounded() {
        assert_eq!(slugify("Fix the OODA loop!"), "fix-the-ooda-loop");
        assert_eq!(slugify("  spaced  "), "spaced");
        assert!(slugify(&"x".repeat(200)).len() <= 48);
    }

    #[test]
    fn refuse_deployer_never_deploys_but_reports_running_commit() {
        let d = RefuseDeployer;
        assert!(d.deploy("abc123").is_err(), "never a blind deploy");
        assert!(d.deployed_commit().is_ok());
    }

    #[test]
    fn intervention_labels_are_stable_for_tracing() {
        // Guard the labels the tracing/tally path depends on.
        assert_eq!(
            Intervention::Report.label(),
            "report",
            "label churn would break tracing dashboards"
        );
    }
}
