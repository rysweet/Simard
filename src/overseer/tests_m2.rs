//! M2 integration fixture (exit criterion): seed a process problem → decide to
//! launch a fix → poll the workstream to its PR → verify + merge the green PR
//! through the gated authority (no `--admin`) → fire the mandatory operator
//! notification on BOTH channels. All with injected fakes — no subprocess, no
//! network, no sleeps.

use std::sync::{Arc, Mutex};

use crate::overseer::capabilities::{
    ObservedState, OverseerError, PrOps, RecipeBrief, RecipeLauncher, WorkstreamHandle,
    WorkstreamStatus,
};
use crate::overseer::intervention::Intervention;
use crate::overseer::launch::{RecipeRunner, SmartOrchestratorLauncher};
use crate::overseer::merge_ops::{DiffReviewer, MergePrOps, PollClock, PollConfig, PrSource};
use crate::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};
use crate::overseer::signal::signals_from;
use crate::overseer::{decide, orient};

use crate::review_pipeline::ReviewFinding;
use crate::stewardship::merge_authority::CheckRollupEntry;
use crate::stewardship::{
    JudgeOutcome, MergeJudge, MergeJudgeKind, PrGhClient, PrSnapshot, Verdict,
};

// ── recipe runner fake ───────────────────────────────────────────────────────

struct FakeRunner {
    repo: String,
    pr: u32,
}
impl RecipeRunner for FakeRunner {
    fn spawn(&self, _brief: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
        Ok(WorkstreamHandle {
            id: "ws-fixture".to_string(),
        })
    }
    fn probe(&self, _h: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError> {
        Ok(WorkstreamStatus::ProducedPr {
            repo: self.repo.clone(),
            pr: self.pr,
        })
    }
}

// ── merge fakes ──────────────────────────────────────────────────────────────

struct GreenGh {
    merges: Mutex<usize>,
}
impl GreenGh {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            merges: Mutex::new(0),
        })
    }
    fn merges(&self) -> usize {
        *self.merges.lock().unwrap()
    }
}
impl PrGhClient for Arc<GreenGh> {
    fn view_pr(&self, _repo: &str, _pr: u32) -> crate::error::SimardResult<PrSnapshot> {
        Ok(PrSnapshot {
            body: "fixture".to_string(),
            mergeable: "MERGEABLE".to_string(),
            review_decision: "APPROVED".to_string(),
            checks: vec![CheckRollupEntry {
                name: "build".to_string(),
                state: "SUCCESS".to_string(),
            }],
            base_ref_name: "main".to_string(),
            labels: Vec::new(),
        })
    }
    fn squash_merge(&self, _repo: &str, _pr: u32) -> crate::error::SimardResult<()> {
        *self.merges.lock().unwrap() += 1;
        Ok(())
    }
}

struct CleanSource;
impl PrSource for CleanSource {
    fn diff(&self, _repo: &str, _pr: u32) -> Result<String, OverseerError> {
        Ok("+++ b/src/overseer/x.rs\n@@ -0,0 +1,1 @@\n+pub fn reasoner() {}\n".to_string())
    }
    fn title(&self, _repo: &str, _pr: u32) -> Result<String, OverseerError> {
        Ok("fix(distill): strip launch-banner noise".to_string())
    }
}

struct NoFindings;
impl DiffReviewer for NoFindings {
    fn review(&self, _diff: &str) -> Result<Vec<ReviewFinding>, OverseerError> {
        Ok(vec![])
    }
}

struct ReadyJudge;
impl MergeJudge for ReadyJudge {
    fn judge(
        &self,
        _pr: u32,
        _repo: &str,
        _snap: &PrSnapshot,
    ) -> crate::error::SimardResult<JudgeOutcome> {
        Ok(JudgeOutcome {
            verdict: Verdict::Ready,
            rationale: "fixture ready".to_string(),
            blockers: vec![],
        })
    }
    fn kind(&self) -> MergeJudgeKind {
        MergeJudgeKind::Llm
    }
}

struct NoSleep;
impl PollClock for NoSleep {
    fn sleep(&self, _secs: u64) {}
}

struct Capture {
    name: String,
    seen: Arc<Mutex<Vec<OperatorNotification>>>,
}
impl NotifyChannel for Capture {
    fn name(&self) -> &str {
        &self.name
    }
    fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
        self.seen.lock().unwrap().push(n.clone());
        ChannelDelivery::Sent
    }
}

#[allow(clippy::type_complexity)]
fn green_merge_ops(
    gh: Arc<GreenGh>,
) -> (
    MergePrOps,
    Arc<Mutex<Vec<OperatorNotification>>>,
    Arc<Mutex<Vec<OperatorNotification>>>,
) {
    let email = Arc::new(Mutex::new(vec![]));
    let signal = Arc::new(Mutex::new(vec![]));
    let notifier = DualChannelNotifier::new(vec![
        Box::new(Capture {
            name: "email".to_string(),
            seen: email.clone(),
        }),
        Box::new(Capture {
            name: "signal".to_string(),
            seen: signal.clone(),
        }),
    ]);
    let ops = MergePrOps::new(
        Box::new(gh),
        Box::new(CleanSource),
        Some(Box::new(NoFindings)),
        Box::new(ReadyJudge),
        notifier,
        Box::new(NoSleep),
        vec!["main".to_string()],
        PollConfig {
            max_attempts: 3,
            interval_secs: 1,
        },
    );
    (ops, email, signal)
}

// ── the fixture ──────────────────────────────────────────────────────────────

#[test]
fn seeded_problem_launches_fix_merges_green_pr_and_notifies_operator() {
    // 1. Observe a process problem (the real ~62% distill-failure case) and let
    //    the full decide() choose to launch a fix workstream.
    let observed = ObservedState {
        distill_fail_pct: Some(62.0),
        ..ObservedState::default()
    };
    let problems = orient(&signals_from(&observed), &[]);
    assert_eq!(problems.len(), 1, "one process-health problem");
    let brief = match decide(&problems[0]) {
        Intervention::LaunchRecipe { brief } => brief,
        other => panic!("process health must launch a recipe, got {other:?}"),
    };
    assert!(brief.task_description.contains("distill"));

    // 2. Launch the fix through the launcher seam and poll it to a PR.
    let launcher = SmartOrchestratorLauncher::new(Box::new(FakeRunner {
        repo: "rysweet/Simard".to_string(),
        pr: 2601,
    }));
    let handle = launcher.launch(&brief).expect("launch");
    let (repo, pr) = match launcher.poll(&handle).expect("poll") {
        WorkstreamStatus::ProducedPr { repo, pr } => (repo, pr),
        other => panic!("expected a PR, got {other:?}"),
    };

    // 3. Verify + merge the green PR through the gated authority; the mandatory
    //    operator notification must fire on BOTH channels.
    let gh = GreenGh::new();
    let (ops, email, signal) = green_merge_ops(gh.clone());
    let report = ops.verify(&repo, pr).expect("verify");
    assert!(report.ready, "green additive PR verifies ready: {report:?}");
    ops.merge(&repo, pr).expect("merge the green PR");

    assert_eq!(gh.merges(), 1, "merged exactly once (squash, no --admin)");
    assert_eq!(email.lock().unwrap().len(), 1, "operator emailed on merge");
    assert_eq!(
        signal.lock().unwrap().len(),
        1,
        "operator Signalled on merge"
    );
    let n = &email.lock().unwrap()[0];
    assert!(n.link.as_deref().unwrap().ends_with("/pull/2601"));
    assert!(
        n.problem.contains("passed every check and review"),
        "the operator notification must be plain English, not gate jargon"
    );
    assert!(n.autonomous);
}

#[test]
fn overseer_gate_holds_verify_merge_until_opt_in() {
    // The Overseer's autonomy gate refuses VerifyAndMergePr by default (crusty
    // risk #1) and admits it only after the operator opts in.
    use crate::overseer::guardrails::AutonomyGate;
    let iv = Intervention::VerifyAndMergePr {
        repo: "rysweet/Simard".to_string(),
        pr: 2601,
    };
    assert!(AutonomyGate::default().admit(&iv).is_err());
    assert!(
        AutonomyGate {
            allow_verify_merge: true,
            allow_high_risk: false,
        }
        .admit(&iv)
        .is_ok()
    );
}
