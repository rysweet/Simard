//! # Overseer — an autonomous operator/observer co-process (DESIGN SKETCH)
//!
//! The `Overseer` embeds the operator/observer role a human+Copilot pair has
//! performed over many sessions: it watches HOW Simard performs, spots problems,
//! and drives improvements **outside** Simard's own OODA workstreams. Simard's
//! OODA governs the external repos she stewards plus her own feature work; the
//! Overseer works at the **meta level** — improving Simard's own health/process
//! and driving cross-cutting initiatives.
//!
//! ## Status: design + scaffolding only
//!
//! This module is a **type/trait sketch**. It is additive, `#![allow(dead_code)]`,
//! and **not wired into `main`** or the daemon loop — nothing here is constructed
//! or scheduled at runtime. It exists to pin down the vocabulary (`Signal`,
//! `Problem`, `Intervention`), the capability seam (`capabilities`), and the
//! guardrails (`guardrails`), each annotated with the EXISTING Simard function it
//! reuses. See `docs/design/overseer.md` for the full architecture, the
//! co-process-vs-`CognitiveThread` decision, and the phased roadmap.
//!
//! ## Architecture (summary)
//!
//! The Overseer is a **sibling co-process**, not a `CognitiveThread`. A
//! `CognitiveThread` is given a least-authority `ThreadContext` and is explicitly
//! forbidden a "code path to self_deploy / self_relaunch / redeploy"
//! (`docs/howto/add-a-new-cognitive-thread.md`); the Overseer needs guarded
//! deploy authority and launches long-running recipe/merge work, so it runs as
//! its own supervised task holding capability handles behind guardrails. A thin,
//! read-only `impl CognitiveThread` **sensor** (observe → signals → report → file
//! issue) is a valid M1 packaging and is described in the design doc; the acting
//! Overseer (M2+) is a co-process.
//!
//! ## Meta-OODA loop
//!
//! `run_cycle` implements one turn of the Overseer's OWN OODA, distinct from
//! Simard's repo-facing OODA:
//!
//! - **Observe** — `StatusReader::snapshot` (wraps `crate::status::assemble`) plus
//!   PR/CI/goal state, folded into `ObservedState`, then `signal::signals_from`.
//! - **Orient** — `orient`: classify + prioritise + **dedup against Simard's
//!   in-flight work** (`GoalCurator::in_flight`).
//! - **Decide** — `decide`: choose one `Intervention` per `Problem`.
//! - **Act** — gate (`guardrails`) then dispatch via the reused capability
//!   (`act`). `run_cycle` only PLANS; execution of admitted interventions is the
//!   M2+ seam.

#![allow(dead_code)]

pub mod audit;
pub mod capabilities;
pub mod config;
pub mod conflict;
pub mod deploy;
pub mod guardrails;
pub mod intervention;
pub mod launch;
pub mod meeting_ops;
pub mod merge_ops;
pub mod notify;
pub mod observer;
pub mod pr_verify;
pub mod sensor;
pub mod signal;
pub mod tuning;
pub mod wiring;

#[cfg(test)]
mod tests_m1;
#[cfg(test)]
mod tests_m2;

pub use capabilities::{
    Auditor, Deployer, GoalCurator, IssueFiler, MeetingHost, ObservedState, OrchestratorRunBrief,
    OverseerError, PrOps, RecipeBrief, RecipeLauncher, StatusReader,
};
pub use config::{
    daily_budget_usd, overseer_acting_enabled, overseer_author_login, overseer_enabled,
};
pub use guardrails::{
    AutonomyGate, BudgetGate, ConflictSequencer, RecursionGuard, RiskClass, Subject, classify,
};
pub use intervention::{Intervention, PlannedIntervention};
pub use observer::{StewardshipIssueFiler, decide_read_only, is_m1_permitted};
pub use sensor::{
    ObserverReport, OverseerSensorThread, SnapshotSource, SnapshotStatusReader,
    in_flight_from_board, observed_from_snapshot, run_observer_cycle,
};
pub use signal::{Priority, Problem, ProblemKind, Signal, signals_from};
pub use wiring::{
    BoardGoalCurator, OverseerCadence, OverseerTickReport, RefuseDeployer, assemble_capabilities,
    build_overseer, overseer_identity, overseer_tick, overseer_tick_interval_secs,
    run_overseer_tick_isolated,
};

use capabilities::{DeployReport, GoalBrief, InFlightItem, IssueOutcome, WorkstreamHandle};

/// The capability handles the Overseer acts through — one per reused Simard
/// subsystem. Grouping them keeps the `Overseer` constructor to a single
/// argument and makes every external dependency explicit and injectable (fakes
/// in tests, real adapters in the daemon).
pub struct Capabilities {
    pub status: Box<dyn StatusReader>,
    pub recipes: Box<dyn RecipeLauncher>,
    pub prs: Box<dyn PrOps>,
    pub deployer: Box<dyn Deployer>,
    pub meetings: Box<dyn MeetingHost>,
    pub issues: Box<dyn IssueFiler>,
    pub goals: Box<dyn GoalCurator>,
    pub auditor: Box<dyn Auditor>,
}

/// The Overseer co-process. Holds its capability handles plus its guardrails.
pub struct Overseer {
    caps: Capabilities,
    autonomy: AutonomyGate,
    recursion: RecursionGuard,
    budget: BudgetGate,
    sequencer: ConflictSequencer,
    /// Cap on how many cost-bearing launches one cycle may plan (concurrency
    /// bound layered on top of the AIMD engineer cap the launcher already obeys).
    max_launches_per_cycle: usize,
}

/// The result of one meta-OODA turn. Side-effect free: it reports what was
/// observed and what WOULD be done. Act (M2+) executes only the admitted items.
#[derive(Clone, Debug, PartialEq)]
pub struct CycleReport {
    pub observed: ObservedState,
    pub signals: Vec<Signal>,
    pub problems: Vec<Problem>,
    pub plan: Vec<PlannedIntervention>,
}

/// Outcome of dispatching one intervention through its capability. Returned by
/// `act` (the M2+ execution seam), exercised here only in tests with fakes.
#[derive(Clone, Debug, PartialEq)]
pub enum ActOutcome {
    Launched(WorkstreamHandle),
    Merged,
    ConflictResolved,
    Deployed(DeployReport),
    IssueFiled(IssueOutcome),
    GoalTransferred,
    Reported,
    Audited,
    Escalated,
}

impl Overseer {
    /// Construct with default guardrails (HIGH-RISK gated, default daily budget).
    pub fn new(caps: Capabilities) -> Self {
        Self {
            caps,
            autonomy: AutonomyGate::default(),
            recursion: RecursionGuard::default(),
            budget: BudgetGate::default(),
            sequencer: ConflictSequencer::default(),
            max_launches_per_cycle: 2,
        }
    }

    /// Opt into autonomous HIGH-RISK execution (deploy / conflict-resolution).
    /// Off by default: those interventions escalate instead.
    pub fn with_high_risk_autonomy(mut self, allow: bool) -> Self {
        self.autonomy.allow_high_risk = allow;
        self
    }

    /// Opt into autonomous PR verify-and-merge (crusty risk #1). Off by default:
    /// `VerifyAndMergePr` escalates until the operator explicitly enables it,
    /// once M1's signal quality is proven. Independent of HIGH-RISK autonomy.
    pub fn with_verify_merge_autonomy(mut self, allow: bool) -> Self {
        self.autonomy.allow_verify_merge = allow;
        self
    }

    /// Set the Overseer's own identity so anti-recursion can refuse its own work.
    pub fn with_identity(mut self, guard: RecursionGuard) -> Self {
        self.recursion = guard;
        self
    }

    /// Run one meta-OODA turn. Observe → Orient → Decide → plan+gate. Does NOT
    /// execute side effects; returns the plan for M2+ Act to run.
    pub fn run_cycle(&mut self) -> Result<CycleReport, OverseerError> {
        // Observe.
        let observed = self.caps.status.snapshot()?;
        let signals = signals_from(&observed);

        // Orient (dedup against Simard's in-flight work; failure to read the
        // board degrades to "no dedup", never aborts the cycle).
        let in_flight = self.caps.goals.in_flight().unwrap_or_default();
        let problems = orient(&signals, &in_flight);

        // Decide + gate.
        let mut plan = Vec::new();
        let mut launches = 0usize;
        for problem in &problems {
            let iv = decide(problem);
            let planned = self.gate(&iv, &observed, &mut launches);
            plan.push(planned);
        }

        Ok(CycleReport {
            observed,
            signals,
            problems,
            plan,
        })
    }

    /// Apply autonomy, budget, and conflict gates to one intervention, producing
    /// a `PlannedIntervention` (admitted or held-with-reason).
    fn gate(
        &mut self,
        iv: &Intervention,
        observed: &ObservedState,
        launches: &mut usize,
    ) -> PlannedIntervention {
        // Autonomy: HIGH-RISK requires opt-in, else it is escalated (held).
        if let Err(e) = self.autonomy.admit(iv) {
            return PlannedIntervention {
                intervention: iv.clone(),
                admitted: false,
                note: e.to_string(),
            };
        }

        // Budget + concurrency: only for cost-bearing launches/audits.
        if is_cost_bearing(iv) {
            if *launches >= self.max_launches_per_cycle {
                return PlannedIntervention {
                    intervention: iv.clone(),
                    admitted: false,
                    note: "held: per-cycle launch cap reached".to_string(),
                };
            }
            if let Some(spent) = observed.spent_today_usd
                && let Err(e) = self.budget.admit(spent)
            {
                return PlannedIntervention {
                    intervention: iv.clone(),
                    admitted: false,
                    note: e.to_string(),
                };
            }
            // Conflict-avoidance: serialise sweeps sharing a sequence group.
            if let Intervention::LaunchRecipe { brief } = iv
                && let Err(e) = self.sequencer.admit(brief.sequence_group.as_deref())
            {
                return PlannedIntervention {
                    intervention: iv.clone(),
                    admitted: false,
                    note: e.to_string(),
                };
            }
            *launches += 1;
        }

        PlannedIntervention {
            intervention: iv.clone(),
            admitted: true,
            note: "admitted".to_string(),
        }
    }

    /// Execute one admitted intervention by dispatching to its reused capability.
    /// This is the M2+ Act seam. Anti-recursion is applied per-subject before any
    /// PR/deploy action.
    pub fn act(&mut self, iv: &Intervention) -> Result<ActOutcome, OverseerError> {
        match iv {
            Intervention::LaunchRecipe { brief } => {
                Ok(ActOutcome::Launched(self.caps.recipes.launch(brief)?))
            }
            Intervention::VerifyAndMergePr { repo, pr } => {
                let report = self.caps.prs.verify(repo, *pr)?;
                if report.ready {
                    self.caps.prs.merge(repo, *pr)?;
                    Ok(ActOutcome::Merged)
                } else {
                    Ok(ActOutcome::Escalated)
                }
            }
            Intervention::ResolveConflict { repo, pr } => {
                self.caps.prs.resolve_conflict(repo, *pr)?;
                Ok(ActOutcome::ConflictResolved)
            }
            Intervention::Deploy { commit } => {
                Ok(ActOutcome::Deployed(self.caps.deployer.deploy(commit)?))
            }
            Intervention::FileIssue { run } => {
                Ok(ActOutcome::IssueFiled(self.caps.issues.file(run)?))
            }
            Intervention::TransferGoal { goal } => {
                self.caps.meetings.transfer_goal(goal)?;
                Ok(ActOutcome::GoalTransferred)
            }
            Intervention::Report => Ok(ActOutcome::Reported),
            Intervention::RunAudit { scope } => {
                self.caps.auditor.run_audit(scope)?;
                Ok(ActOutcome::Audited)
            }
            Intervention::Escalate { .. } => Ok(ActOutcome::Escalated),
        }
    }
}

/// True for interventions that spend LLM budget / spawn work.
fn is_cost_bearing(iv: &Intervention) -> bool {
    matches!(
        iv,
        Intervention::LaunchRecipe { .. } | Intervention::RunAudit { .. }
    )
}

/// Orient: fold `Signal`s into ranked, deduplicated `Problem`s. Dedups against
/// Simard's in-flight work (so the Overseer never fights an engineer already on
/// the case) and against problems already collected this cycle.
pub fn orient(signals: &[Signal], in_flight: &[InFlightItem]) -> Vec<Problem> {
    let mut problems: Vec<Problem> = Vec::new();

    for s in signals {
        let (kind, priority, key, summary) = classify_signal(s);

        // Dedup against Simard's in-flight work.
        if in_flight.iter().any(|i| i.refs.iter().any(|r| r == &key)) {
            continue;
        }
        // Merge into an existing same-key problem rather than duplicating.
        if let Some(existing) = problems.iter_mut().find(|p| p.dedup_key == key) {
            existing.evidence.push(s.clone());
            continue;
        }
        problems.push(Problem {
            kind,
            priority,
            dedup_key: key,
            summary,
            evidence: vec![s.clone()],
        });
    }

    problems.sort_by_key(|p| p.priority);
    problems
}

/// Map a single `Signal` to `(kind, priority, dedup_key, summary)`.
fn classify_signal(s: &Signal) -> (ProblemKind, Priority, String, String) {
    match s {
        Signal::DistillFailureRate { pct } => (
            ProblemKind::ProcessHealth,
            Priority::High,
            "process:distill_fail".to_string(),
            format!("distillation parse-failure rate {pct:.0}%"),
        ),
        Signal::RestartChurn { restarts } => (
            ProblemKind::ProcessHealth,
            Priority::High,
            "process:restart_churn".to_string(),
            format!("daemon restart churn ({restarts} restarts)"),
        ),
        Signal::LadderExhausted { count } => (
            ProblemKind::ProcessHealth,
            Priority::Normal,
            "process:ladder_exhausted".to_string(),
            format!("reasoner decide-ladder exhausted ({count})"),
        ),
        Signal::BudgetPressure {
            spent_usd,
            budget_usd,
        } => (
            ProblemKind::ResourcePressure,
            Priority::High,
            "resource:budget".to_string(),
            format!("LLM budget pressure (${spent_usd:.2} of ${budget_usd:.2})"),
        ),
        Signal::EngineerSpawnRate { live } => (
            ProblemKind::ResourcePressure,
            Priority::Normal,
            "resource:engineer_spawn".to_string(),
            format!("elevated engineer spawn ({live} live)"),
        ),
        Signal::MemoryGrowth { nodes_total } => (
            ProblemKind::ResourcePressure,
            Priority::Low,
            "resource:memory_growth".to_string(),
            format!("cognitive-memory growth ({nodes_total} nodes)"),
        ),
        Signal::GymSkipped => (
            ProblemKind::QualityRegression,
            Priority::Low,
            "quality:gym_skipped".to_string(),
            "gym self-eval skipped".to_string(),
        ),
        Signal::CiFailureCluster { repo, failing } => (
            ProblemKind::QualityRegression,
            Priority::High,
            format!("quality:ci:{repo}"),
            format!("CI-failure cluster in {repo} ({failing} failing)"),
        ),
        Signal::PrReadyToMerge { repo, pr } => (
            ProblemKind::DeliveryReady,
            Priority::Normal,
            format!("delivery:pr:{repo}#{pr}"),
            format!("PR {repo}#{pr} is green and merge-ready"),
        ),
        Signal::StaleGoal { goal_id } => (
            ProblemKind::GoalHygiene,
            Priority::Normal,
            format!("goal:stale:{goal_id}"),
            format!("goal {goal_id} re-litigated / stale-complete"),
        ),
        Signal::Anomaly { detail } => (
            ProblemKind::ProcessHealth,
            Priority::Normal,
            format!("anomaly:{detail}"),
            format!("telemetry anomaly: {detail}"),
        ),
    }
}

/// Decide: choose one `Intervention` for a `Problem`. Illustrative routing; a
/// production Overseer would use a prompt-driven reasoner with this deterministic
/// mapping as its floor (mirroring `OodaDecideBrain`'s deterministic fallback).
pub fn decide(problem: &Problem) -> Intervention {
    match problem.kind {
        ProblemKind::DeliveryReady => {
            for s in &problem.evidence {
                if let Signal::PrReadyToMerge { repo, pr } = s {
                    return Intervention::VerifyAndMergePr {
                        repo: repo.clone(),
                        pr: *pr,
                    };
                }
            }
            Intervention::Report
        }
        ProblemKind::QualityRegression => {
            for s in &problem.evidence {
                if let Signal::CiFailureCluster { repo, failing } = s {
                    return Intervention::FileIssue {
                        run: OrchestratorRunBrief {
                            recipe_name: "smart-orchestrator".to_string(),
                            failed_step: "ci".to_string(),
                            source_module: repo.clone(),
                            failure_kind: "ci_failure_cluster".to_string(),
                            error_text: format!("{failing} failing checks in {repo}"),
                        },
                    };
                }
            }
            Intervention::Report
        }
        ProblemKind::ProcessHealth => Intervention::LaunchRecipe {
            brief: RecipeBrief {
                task_description: problem.summary.clone(),
                target_repo: "rysweet/Simard".to_string(),
                sequence_group: None,
            },
        },
        ProblemKind::CrossCutting => Intervention::LaunchRecipe {
            brief: RecipeBrief {
                task_description: problem.summary.clone(),
                target_repo: "rysweet/Simard".to_string(),
                // Mechanical sweeps on shared OODA-core files serialise here.
                sequence_group: Some("ooda-core".to_string()),
            },
        },
        ProblemKind::ResourcePressure => Intervention::Escalate {
            reason: problem.summary.clone(),
        },
        ProblemKind::GoalHygiene => Intervention::TransferGoal {
            goal: GoalBrief {
                title: problem.summary.clone(),
                rationale: "stale / re-litigated goal — transfer to Simard for closure".to_string(),
                priority: 3,
                target_repo: "rysweet/Simard".to_string(),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::capabilities::*;
    use super::*;

    // ── Fakes: each satisfies one capability with canned values. ────────────
    struct FakeStatus(ObservedState);
    impl StatusReader for FakeStatus {
        fn snapshot(&self) -> Result<ObservedState, OverseerError> {
            Ok(self.0.clone())
        }
    }

    struct FakeRecipes;
    impl RecipeLauncher for FakeRecipes {
        fn launch(&self, _brief: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
            Ok(WorkstreamHandle {
                id: "ws-1".to_string(),
            })
        }
        fn poll(&self, _h: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError> {
            Ok(WorkstreamStatus::Running)
        }
    }

    struct FakePrs {
        ready: bool,
    }
    impl PrOps for FakePrs {
        fn verify(&self, _repo: &str, _pr: u32) -> Result<VerifyReport, OverseerError> {
            Ok(VerifyReport {
                ready: self.ready,
                checks: vec![],
            })
        }
        fn merge(&self, _repo: &str, _pr: u32) -> Result<(), OverseerError> {
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
    impl MeetingHost for FakeMeetings {
        fn transfer_goal(&self, _goal: &GoalBrief) -> Result<(), OverseerError> {
            Ok(())
        }
    }

    struct FakeIssues;
    impl IssueFiler for FakeIssues {
        fn file(&self, _run: &OrchestratorRunBrief) -> Result<IssueOutcome, OverseerError> {
            Ok(IssueOutcome::FiledNew {
                url: "https://example/issues/1".to_string(),
            })
        }
    }

    struct FakeGoals(Vec<InFlightItem>);
    impl GoalCurator for FakeGoals {
        fn propose(&self, _goal: &GoalBrief) -> Result<(), OverseerError> {
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

    fn caps(observed: ObservedState, ready: bool, in_flight: Vec<InFlightItem>) -> Capabilities {
        Capabilities {
            status: Box::new(FakeStatus(observed)),
            recipes: Box::new(FakeRecipes),
            prs: Box::new(FakePrs { ready }),
            deployer: Box::new(FakeDeployer),
            meetings: Box::new(FakeMeetings),
            issues: Box::new(FakeIssues),
            goals: Box::new(FakeGoals(in_flight)),
            auditor: Box::new(FakeAuditor),
        }
    }

    #[test]
    fn signals_only_fire_above_threshold() {
        let below = ObservedState {
            distill_fail_pct: Some(5.0),
            ..ObservedState::default()
        };
        assert!(signals_from(&below).is_empty());
        // the real-world ~62% case
        let high = ObservedState {
            distill_fail_pct: Some(62.0),
            ..ObservedState::default()
        };
        let sigs = signals_from(&high);
        assert_eq!(sigs, vec![Signal::DistillFailureRate { pct: 62.0 }]);
    }

    #[test]
    fn orient_dedups_against_in_flight() {
        let signals = vec![Signal::DistillFailureRate { pct: 62.0 }];
        // An engineer is already on it (same dedup key) → no problem raised.
        let in_flight = vec![InFlightItem {
            id: "g1".to_string(),
            source: "ooda".to_string(),
            refs: vec!["process:distill_fail".to_string()],
        }];
        assert!(orient(&signals, &in_flight).is_empty());
        // Nobody on it → one problem.
        assert_eq!(orient(&signals, &[]).len(), 1);
    }

    #[test]
    fn run_cycle_plans_a_launch_for_process_health() {
        let st = ObservedState {
            distill_fail_pct: Some(62.0),
            ..ObservedState::default()
        };
        let mut ov = Overseer::new(caps(st, true, vec![]));
        let report = ov.run_cycle().expect("cycle");
        assert_eq!(report.problems.len(), 1);
        assert_eq!(report.plan.len(), 1);
        let planned = &report.plan[0];
        assert!(planned.admitted);
        assert_eq!(planned.intervention.label(), "launch_recipe");
    }

    #[test]
    fn high_risk_deploy_is_gated_by_default() {
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]));
        // Default autonomy holds a deploy.
        let held = ov.gate(
            &Intervention::Deploy {
                commit: "abc123".to_string(),
            },
            &ObservedState::default(),
            &mut 0,
        );
        assert!(!held.admitted);
        // Opt-in autonomy admits it.
        let mut ov = ov.with_high_risk_autonomy(true);
        let admitted = ov.gate(
            &Intervention::Deploy {
                commit: "abc123".to_string(),
            },
            &ObservedState::default(),
            &mut 0,
        );
        assert!(admitted.admitted);
    }

    #[test]
    fn budget_pressure_holds_launches() {
        let observed = ObservedState {
            spent_today_usd: Some(600.0),
            daily_budget_usd: Some(500.0),
            ..ObservedState::default()
        };
        let mut ov = Overseer::new(caps(observed.clone(), true, vec![]));
        let held = ov.gate(
            &Intervention::LaunchRecipe {
                brief: RecipeBrief {
                    task_description: "x".to_string(),
                    target_repo: "rysweet/Simard".to_string(),
                    sequence_group: None,
                },
            },
            &observed,
            &mut 0,
        );
        assert!(!held.admitted);
    }

    #[test]
    fn anti_recursion_refuses_own_pr() {
        let guard = RecursionGuard {
            author_login: "simard-overseer[bot]".to_string(),
            branch_prefix: "overseer/".to_string(),
            goal_source_tag: "overseer:".to_string(),
        };
        let own = Subject::Pr {
            repo: "rysweet/Simard".to_string(),
            pr: 1,
            author: "simard-overseer[bot]".to_string(),
        };
        assert!(guard.is_own(&own));
        let foreign = Subject::Pr {
            repo: "rysweet/Simard".to_string(),
            pr: 2,
            author: "someone-else".to_string(),
        };
        assert!(!guard.is_own(&foreign));
    }

    #[test]
    fn conflict_sequencer_serialises_sweeps() {
        let mut seq = ConflictSequencer::default();
        assert!(seq.admit(Some("ooda-core")).is_ok());
        // A second sweep on the same shared files is held until the first frees.
        assert!(seq.admit(Some("ooda-core")).is_err());
        seq.release("ooda-core");
        assert!(seq.admit(Some("ooda-core")).is_ok());
        // Unsequenced feature work is always admitted.
        assert!(seq.admit(None).is_ok());
    }

    #[test]
    fn act_dispatches_merge_when_ready() {
        let mut ov = Overseer::new(caps(ObservedState::default(), true, vec![]));
        let out = ov
            .act(&Intervention::VerifyAndMergePr {
                repo: "rysweet/Simard".to_string(),
                pr: 7,
            })
            .expect("act");
        assert_eq!(out, ActOutcome::Merged);
    }

    #[test]
    fn act_escalates_merge_when_not_ready() {
        let mut ov = Overseer::new(caps(ObservedState::default(), false, vec![]));
        let out = ov
            .act(&Intervention::VerifyAndMergePr {
                repo: "rysweet/Simard".to_string(),
                pr: 7,
            })
            .expect("act");
        assert_eq!(out, ActOutcome::Escalated);
    }
}
