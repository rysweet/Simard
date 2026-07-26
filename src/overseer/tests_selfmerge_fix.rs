//! TDD (Step 7) — FAILING tests that pin the gated self-merge fix
//! (#4097 / #4100). Simard's autonomous self-merge has NEVER merged a PR
//! because `verify()`'s review gate (#7) is fail-closed whenever no
//! `DiffReviewer` is wired — and one is NEVER wired in production. Every
//! survey candidate therefore escalates and `prs_merged` is 0 forever.
//!
//! The fix (Design **b**): REMOVE the code-based review gate + `DiffReviewer`
//! from `verify()`. `verify()` becomes an OBJECTIVE pre-filter (objective gates
//! plus deterministic diff-scans) whose `ready == true` means "eligible to proceed
//! to the authoritative merge", NOT "approved". The single, authoritative review
//! authority is the ALREADY-WIRED agentic `MergeJudge`
//! (`prompt_assets/…/merge_readiness_judge.md`) invoked at `merge()` step 3.
//! When the judge refuses (or no LLM provider is configured, so the fallback
//! `RefusingMergeJudge` is used), `merge()` returns the new
//! `OverseerError::NotMergeReady` variant, which the Act handler maps to an
//! escalation (never an error, never a blind merge).
//!
//! These tests reference the TARGET API and MUST fail to compile / fail to pass
//! against the current tree (the 8-arg `MergePrOps::new` with a reviewer, the
//! fail-closed check #7, and the missing `NotMergeReady` variant). They go GREEN
//! only once the fix lands.

use std::sync::{Arc, Mutex};

use crate::overseer::capabilities::{OverseerError, PrOps, PrRef, VerifyReport};
use crate::overseer::merge_ops::{MergePrOps, PollClock, PollConfig, PrSource};
use crate::overseer::notify::{
    ChannelDelivery, DualChannelNotifier, NotifyChannel, OperatorNotification,
};
use crate::stewardship::merge_authority::{CheckRollupEntry, OpenPrSummary};
use crate::stewardship::{
    JudgeOutcome, MergeJudge, MergeJudgeKind, PrGhClient, PrSnapshot, RefusingMergeJudge, Verdict,
};

// ─────────────────────────── fakes ──────────────────────────────────────────

/// A `PrGhClient` that serves ONE scripted snapshot for every `view_pr`
/// (clamped, so a single entry is reused across verify + poll + authority) and
/// records `squash_merge` calls. The trait has no `--admin` method, so the
/// no-admin / squash-only guarantee is structural.
struct ScriptedGh {
    snapshot: PrSnapshot,
    merges: Mutex<usize>,
}
impl ScriptedGh {
    fn new(snapshot: PrSnapshot) -> Arc<Self> {
        Arc::new(Self {
            snapshot,
            merges: Mutex::new(0),
        })
    }
    fn merges(&self) -> usize {
        *self.merges.lock().unwrap()
    }
}
impl PrGhClient for Arc<ScriptedGh> {
    fn view_pr(&self, _repo: &str, _pr: u32) -> crate::error::SimardResult<PrSnapshot> {
        Ok(self.snapshot.clone())
    }
    fn squash_merge(&self, _repo: &str, _pr: u32) -> crate::error::SimardResult<()> {
        *self.merges.lock().unwrap() += 1;
        Ok(())
    }
}

/// PR text source with a configurable diff + author (the author drives the
/// merge-step anti-recursion / author re-assert).
struct FakeSource {
    diff: String,
    author: String,
}
impl PrSource for FakeSource {
    fn diff(&self, _repo: &str, _pr: u32) -> Result<String, OverseerError> {
        Ok(self.diff.clone())
    }
    fn title(&self, _repo: &str, _pr: u32) -> Result<String, OverseerError> {
        Ok("fix(engineer): tidy an additive helper".to_string())
    }
    fn author(&self, _repo: &str, _pr: u32) -> Result<String, OverseerError> {
        Ok(self.author.clone())
    }
}

/// The agentic judge stubbed to APPROVE (stands in for the LLM-backed
/// `merge_readiness_judge.md` verdict on a genuinely merge-ready PR).
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
            rationale: "stub: merge-ready".to_string(),
            blockers: vec![],
        })
    }
    fn kind(&self) -> MergeJudgeKind {
        MergeJudgeKind::Llm
    }
}

/// The agentic judge stubbed to REFUSE (stands in for the LLM judging a PR
/// NOT merge-ready). A refusal MUST escalate, never merge.
struct RefuseJudge;
impl MergeJudge for RefuseJudge {
    fn judge(
        &self,
        _pr: u32,
        _repo: &str,
        _snap: &PrSnapshot,
    ) -> crate::error::SimardResult<JudgeOutcome> {
        Ok(JudgeOutcome {
            verdict: Verdict::NotReady,
            rationale: "stub: not merge-ready".to_string(),
            blockers: vec![],
        })
    }
    fn kind(&self) -> MergeJudgeKind {
        MergeJudgeKind::Llm
    }
}

/// Counts sleeps instead of sleeping (no real sleep in tests).
#[derive(Default)]
struct CountingClock {
    sleeps: Mutex<usize>,
}
impl PollClock for Arc<CountingClock> {
    fn sleep(&self, _secs: u64) {
        *self.sleeps.lock().unwrap() += 1;
    }
}

/// Records every notification handed to it.
struct CapturingChannel {
    name: String,
    seen: Arc<Mutex<Vec<OperatorNotification>>>,
}
impl NotifyChannel for CapturingChannel {
    fn name(&self) -> &str {
        &self.name
    }
    fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
        self.seen.lock().unwrap().push(n.clone());
        ChannelDelivery::Sent
    }
}

// ─────────────────────────── snapshot / helpers ─────────────────────────────

fn check(name: &str, state: &str) -> CheckRollupEntry {
    CheckRollupEntry {
        name: name.to_string(),
        state: state.to_string(),
    }
}

fn snapshot(mergeable: &str, checks: Vec<CheckRollupEntry>, labels: Vec<String>) -> PrSnapshot {
    PrSnapshot {
        body: "body".to_string(),
        mergeable: mergeable.to_string(),
        review_decision: "REVIEW_REQUIRED".to_string(),
        checks,
        base_ref_name: "main".to_string(),
        labels,
        is_draft: Some(false),
    }
}

/// A green + MERGEABLE snapshot targeting `main`, with NO human/GitHub review
/// approval recorded (`REVIEW_REQUIRED`) — proving the merge does NOT depend on
/// a code-review approval; the agentic judge is the sole review authority.
fn green() -> PrSnapshot {
    snapshot(
        "MERGEABLE",
        vec![check("build", "SUCCESS"), check("clippy", "SUCCESS")],
        vec![],
    )
}

/// An additive, obviously-clean diff (passes the deterministic diff-scans).
const CLEAN_DIFF: &str = "\
+++ b/src/overseer/x.rs
@@ -0,0 +1,2 @@
+pub fn reasoner() {}
+// orient-decide-act
";

const REPO: &str = "rysweet/Simard";
const ENGINEER_AUTHOR: &str = "rysweet";

/// Build a `MergePrOps` on the TARGET (reviewer-free) constructor with capturing
/// notify channels. `judge` is the sole review authority; `author` feeds the
/// merge-step author re-assert (wired via `with_automerge_author`).
#[allow(clippy::type_complexity)]
fn ops_with(
    gh: Arc<ScriptedGh>,
    diff: &str,
    author: &str,
    judge: Box<dyn MergeJudge>,
    clock: Arc<CountingClock>,
    poll: PollConfig,
) -> (
    MergePrOps,
    Arc<Mutex<Vec<OperatorNotification>>>,
    Arc<Mutex<Vec<OperatorNotification>>>,
) {
    let email = Arc::new(Mutex::new(vec![]));
    let signal = Arc::new(Mutex::new(vec![]));
    let notifier = DualChannelNotifier::new(vec![
        Box::new(CapturingChannel {
            name: "email".to_string(),
            seen: email.clone(),
        }),
        Box::new(CapturingChannel {
            name: "signal".to_string(),
            seen: signal.clone(),
        }),
    ]);
    // TARGET SIGNATURE: no reviewer argument — the DiffReviewer wire is removed.
    let ops = MergePrOps::new(
        Box::new(gh),
        Box::new(FakeSource {
            diff: diff.to_string(),
            author: author.to_string(),
        }),
        judge,
        notifier,
        Box::new(clock),
        vec!["main".to_string()],
        poll,
    )
    .with_automerge_author(ENGINEER_AUTHOR.to_string());
    (ops, email, signal)
}

// ─────────────────────── the live-bug regression ────────────────────────────

/// THE regression. A green, MERGEABLE, `main`-targeted engineer PR with an
/// obviously-clean diff must `verify().ready == true` — WITHOUT any reviewer
/// wired. Today this returns `false` (check #7 fail-closes "review unavailable"),
/// which is exactly why 100% of candidates escalate and 0 ever merge.
#[test]
fn regression_green_engineer_pr_verifies_ready_with_no_reviewer_wired() {
    let gh = ScriptedGh::new(green());
    let (ops, _e, _s) = ops_with(
        gh,
        CLEAN_DIFF,
        ENGINEER_AUTHOR,
        Box::new(ReadyJudge),
        Arc::new(CountingClock::default()),
        PollConfig::default(),
    );
    let report = ops.verify(REPO, 4097).expect("verify runs");
    assert!(
        report.ready,
        "a green, mergeable, clean-diff engineer PR must verify READY with no \
         reviewer wired (objective pre-filter); got {report:?}"
    );
}

/// `verify()` is a review-FREE objective pre-filter: none of its checklist items
/// may be the old code-based review gate. This pins that check #7 (and the
/// `DiffReviewer` wire) is gone.
#[test]
fn verify_has_no_code_review_gate_only_objective_and_diffscan_checks() {
    let gh = ScriptedGh::new(green());
    let (ops, _e, _s) = ops_with(
        gh,
        CLEAN_DIFF,
        ENGINEER_AUTHOR,
        Box::new(ReadyJudge),
        Arc::new(CountingClock::default()),
        PollConfig::default(),
    );
    let report = ops.verify(REPO, 1).expect("verify runs");
    assert!(
        report
            .checks
            .iter()
            .all(|c| !c.name.to_lowercase().contains("review")),
        "verify() must carry NO review gate — the agentic MergeJudge is the sole \
         review authority (merge step 3); got {:?}",
        report.checks
    );
    assert!(
        report
            .checks
            .iter()
            .all(|c| !c.note.to_lowercase().contains("no reviewer wired")),
        "the fail-closed 'no reviewer wired' note must be gone entirely"
    );
}

/// End-to-end: the same green engineer PR proceeds through `merge()` to a single
/// squash-merge when the agentic judge approves, and BOTH operator channels are
/// notified in plain English. Today this never happens (verify fail-closes).
#[test]
fn green_engineer_pr_reaches_squash_merge_when_judge_ready() {
    let gh = ScriptedGh::new(green());
    let (ops, email, signal) = ops_with(
        gh.clone(),
        CLEAN_DIFF,
        ENGINEER_AUTHOR,
        Box::new(ReadyJudge),
        Arc::new(CountingClock::default()),
        PollConfig::default(),
    );
    ops.merge(REPO, 4097)
        .expect("green PR + judge-ready merges");
    assert_eq!(gh.merges(), 1, "exactly one squash-merge (no --admin path)");
    assert_eq!(email.lock().unwrap().len(), 1, "operator emailed on merge");
    assert_eq!(
        signal.lock().unwrap().len(),
        1,
        "operator signalled on merge"
    );
    let n = &email.lock().unwrap()[0];
    assert!(
        n.link.as_deref().unwrap().contains("/pull/4097"),
        "the notification links the merged PR"
    );
    assert!(n.autonomous);
    assert!(
        !n.problem.contains("check #7")
            && !n.problem.contains("DiffReviewer")
            && !n.problem.contains("objective gates")
            && !n.problem.contains("pr-verify"),
        "the operator message stays plain English — no internal gate jargon: {:?}",
        n.problem
    );
}

// ──────────────────── judge refusal / provider outage escalate ───────────────

/// A judge REFUSAL (`Verdict::NotReady`) must ESCALATE, never merge: `merge()`
/// returns the new [`OverseerError::NotMergeReady`] variant and performs no
/// squash-merge / no notification.
#[test]
fn merge_escalates_via_not_merge_ready_when_judge_refuses() {
    let gh = ScriptedGh::new(green());
    let (ops, email, signal) = ops_with(
        gh.clone(),
        CLEAN_DIFF,
        ENGINEER_AUTHOR,
        Box::new(RefuseJudge),
        Arc::new(CountingClock::default()),
        PollConfig::default(),
    );
    let err = ops
        .merge(REPO, 4097)
        .expect_err("a judge refusal must not merge");
    assert!(
        matches!(err, OverseerError::NotMergeReady { .. }),
        "a judge refusal must surface as NotMergeReady (→ escalation), got {err:?}"
    );
    assert_eq!(gh.merges(), 0, "no squash-merge on a judge refusal");
    assert!(email.lock().unwrap().is_empty(), "no notify on a refusal");
    assert!(signal.lock().unwrap().is_empty());
}

/// FAIL-CLOSED on LLM-provider outage: with the production fallback
/// [`RefusingMergeJudge`] (no LLM configured), `merge()` must NOT merge and must
/// surface [`OverseerError::NotMergeReady`] so the candidate escalates. The
/// judge outage can never default to "approve".
#[test]
fn merge_fails_closed_as_not_merge_ready_when_no_llm_provider() {
    let gh = ScriptedGh::new(green());
    let (ops, email, _s) = ops_with(
        gh.clone(),
        CLEAN_DIFF,
        ENGINEER_AUTHOR,
        Box::new(RefusingMergeJudge),
        Arc::new(CountingClock::default()),
        PollConfig::default(),
    );
    let err = ops
        .merge(REPO, 4097)
        .expect_err("no LLM provider must fail closed, never merge");
    assert!(
        matches!(err, OverseerError::NotMergeReady { .. }),
        "an unavailable judge (RefusingMergeJudge) must escalate, not merge: {err:?}"
    );
    assert_eq!(gh.merges(), 0, "provider outage must never squash-merge");
    assert!(email.lock().unwrap().is_empty());
}

// ─────────────────── preserved objective safety properties ───────────────────

/// Objective gate preserved: red CI never verifies ready (poll/merge escalates).
#[test]
fn verify_still_not_ready_when_ci_red() {
    let red = snapshot("MERGEABLE", vec![check("build", "FAILURE")], vec![]);
    let gh = ScriptedGh::new(red);
    let (ops, _e, _s) = ops_with(
        gh,
        CLEAN_DIFF,
        ENGINEER_AUTHOR,
        Box::new(ReadyJudge),
        Arc::new(CountingClock::default()),
        PollConfig::default(),
    );
    assert!(
        !ops.verify(REPO, 1).expect("verify runs").ready,
        "a red CI check must keep the PR NOT ready (objective gate preserved)"
    );
}

/// Diff-scan preserved: a non-additive / risky diff never verifies ready.
/// Since #4163 the two redundant style scans are judge-advisory, so the
/// retained merge-safety gate exercised here is the additive one — a
/// removed `pub` item (a breaking-API change) must still fail verify.
#[test]
fn verify_still_not_ready_on_dirty_diff() {
    let dirty = "\
+++ b/src/x.rs
@@ -1,2 +1,1 @@
-pub fn removed_api() {}
 fn keep() {}
";
    let gh = ScriptedGh::new(green());
    let (ops, _e, _s) = ops_with(
        gh,
        dirty,
        ENGINEER_AUTHOR,
        Box::new(ReadyJudge),
        Arc::new(CountingClock::default()),
        PollConfig::default(),
    );
    assert!(
        !ops.verify(REPO, 1).expect("verify runs").ready,
        "the deterministic diff-scans must still fail a risky (non-additive) diff"
    );
}

/// Base allow-list preserved: a PR targeting a non-allowlisted base is NOT ready
/// even when green + mergeable.
#[test]
fn verify_still_not_ready_off_base_allowlist() {
    let mut snap = green();
    snap.base_ref_name = "release/9".to_string();
    let gh = ScriptedGh::new(snap);
    let (ops, _e, _s) = ops_with(
        gh,
        CLEAN_DIFF,
        ENGINEER_AUTHOR,
        Box::new(ReadyJudge),
        Arc::new(CountingClock::default()),
        PollConfig::default(),
    );
    assert!(
        !ops.verify(REPO, 1).expect("verify runs").ready,
        "a non-allowlisted base branch must keep the PR NOT ready"
    );
}

/// Author re-assert preserved: even on a fully green, judge-ready PR, `merge()`
/// refuses a PR authored by someone OTHER than the configured autonomous-merge
/// identity (fail-closed defense-in-depth). This is a genuine safety refusal, so
/// it must NOT merge and must NOT notify.
#[test]
fn merge_still_refuses_foreign_author() {
    let gh = ScriptedGh::new(green());
    let (ops, email, _s) = ops_with(
        gh.clone(),
        CLEAN_DIFF,
        "some-human-operator", // author != configured "rysweet"
        Box::new(ReadyJudge),
        Arc::new(CountingClock::default()),
        PollConfig::default(),
    );
    assert!(
        ops.merge(REPO, 4097).is_err(),
        "a PR whose author is not the configured identity must be refused"
    );
    assert_eq!(gh.merges(), 0, "no merge of a foreign-author PR");
    assert!(email.lock().unwrap().is_empty(), "no notify on refusal");
}

/// Creative-idea-label exclusion stays in the merge authority (merge step 3): a
/// PR carrying the block-until-human-review label is refused as NotMergeReady
/// even though it is green + mergeable + clean-diff.
#[test]
fn merge_still_refuses_creative_idea_label() {
    let snap = snapshot(
        "MERGEABLE",
        vec![check("build", "SUCCESS")],
        vec![crate::creative_ideas::CREATIVE_IDEA_PR_LABEL.to_string()],
    );
    let gh = ScriptedGh::new(snap);
    let (ops, email, _s) = ops_with(
        gh.clone(),
        CLEAN_DIFF,
        ENGINEER_AUTHOR,
        Box::new(ReadyJudge),
        Arc::new(CountingClock::default()),
        PollConfig::default(),
    );
    let err = ops
        .merge(REPO, 4097)
        .expect_err("a creative-idea PR awaiting human review must not merge");
    assert!(
        matches!(err, OverseerError::NotMergeReady { .. }),
        "the creative-idea gate must escalate (NotMergeReady), got {err:?}"
    );
    assert_eq!(gh.merges(), 0, "creative-idea PR never squash-merges");
    assert!(email.lock().unwrap().is_empty());
}

// ─────────────────── survey scoping (#4147) preserved ────────────────────────

/// A `PrGhClient` scripted for the survey rail: `list_prs_by_author` returns a
/// fixed listing; `view_pr` is unreachable (the survey decides from listing
/// fields only) and `squash_merge` is recorded so a test can assert the sensor
/// never merges.
struct SurveyGh {
    listing: Vec<OpenPrSummary>,
    merges: Mutex<usize>,
}
impl SurveyGh {
    fn new(listing: Vec<OpenPrSummary>) -> Arc<Self> {
        Arc::new(Self {
            listing,
            merges: Mutex::new(0),
        })
    }
    fn merges(&self) -> usize {
        *self.merges.lock().unwrap()
    }
}
impl PrGhClient for Arc<SurveyGh> {
    fn view_pr(&self, _repo: &str, _pr: u32) -> crate::error::SimardResult<PrSnapshot> {
        unreachable!("the survey rail must never call view_pr")
    }
    fn squash_merge(&self, _repo: &str, _pr: u32) -> crate::error::SimardResult<()> {
        *self.merges.lock().unwrap() += 1;
        Ok(())
    }
    fn list_prs_by_author(
        &self,
        _repo: &str,
        _author: &str,
        _limit: u32,
    ) -> crate::error::SimardResult<Vec<OpenPrSummary>> {
        Ok(self.listing.clone())
    }
}

fn open_pr(number: u32, author: &str, head: &str, labels: &[&str]) -> OpenPrSummary {
    OpenPrSummary {
        number,
        title: format!("candidate #{number}"),
        head_ref_name: head.to_string(),
        base_ref_name: "main".to_string(),
        mergeable: "MERGEABLE".to_string(),
        checks: vec![check("ci", "SUCCESS")],
        url: format!("https://github.com/rysweet/Simard/pull/{number}"),
        author: author.to_string(),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        // Default fixtures are non-draft so the pre-existing green survey cases
        // stay green; the #4339 draft tests below override this per-case.
        is_draft: Some(false),
    }
}

fn survey_ops(gh: Arc<SurveyGh>, author: Option<&str>) -> MergePrOps {
    let notifier = DualChannelNotifier::new(vec![Box::new(CapturingChannel {
        name: "email".to_string(),
        seen: Arc::new(Mutex::new(vec![])),
    })]);
    // TARGET SIGNATURE: no reviewer argument.
    let mut ops = MergePrOps::new(
        Box::new(gh),
        Box::new(FakeSource {
            diff: String::new(),
            author: ENGINEER_AUTHOR.to_string(),
        }),
        Box::new(ReadyJudge),
        notifier,
        Box::new(Arc::new(CountingClock::default())),
        vec!["main".to_string()],
        PollConfig::default(),
    );
    if let Some(a) = author {
        ops = ops.with_automerge_author(a.to_string());
    }
    ops
}

/// #4147 safety: an operator review PR — SAME author as the engineer identity
/// (`rysweet`), green + mergeable, but NO `simard-autonomous` label and a
/// non-engineer branch (modeled on #3142 `cogthreads/…`) — is NEVER a candidate
/// and the sensor NEVER merges.
#[test]
fn survey_excludes_operator_review_pr_without_label_or_engineer_branch() {
    let gh = SurveyGh::new(vec![
        open_pr(3142, ENGINEER_AUTHOR, "cogthreads/dashboard", &[]),
        open_pr(3200, ENGINEER_AUTHOR, "feat/operator-hotfix", &[]),
    ]);
    let ops = survey_ops(gh.clone(), Some(ENGINEER_AUTHOR));
    assert!(
        ops.survey_ready_prs(&[REPO.to_string()]).is_empty(),
        "an operator review PR (no engineer label, non-engineer branch) must \
         never be a candidate, even when author + CI + mergeable all pass"
    );
    assert_eq!(gh.merges(), 0, "the survey rail must never merge");
}

/// #4147: an engineer PR — same author, on the deterministic `engineer/` branch
/// namespace — IS a candidate even without the label.
#[test]
fn survey_includes_engineer_pr_by_branch_namespace() {
    let gh = SurveyGh::new(vec![open_pr(
        700,
        ENGINEER_AUTHOR,
        "engineer/700-ab12cd34",
        &[],
    )]);
    let ops = survey_ops(gh, Some(ENGINEER_AUTHOR));
    assert_eq!(
        ops.survey_ready_prs(&[REPO.to_string()]),
        vec![PrRef {
            repo: REPO.to_string(),
            pr: 700,
        }],
        "an engineer-branch PR is an eligible candidate"
    );
}

/// #4147: an engineer PR identified by the durable `simard-autonomous` LABEL is
/// a candidate even on a shared branch prefix.
#[test]
fn survey_includes_engineer_pr_by_label() {
    let gh = SurveyGh::new(vec![open_pr(
        701,
        ENGINEER_AUTHOR,
        "feat/shared-prefix",
        &[crate::overseer::config::SIMARD_ENGINEER_PR_LABEL],
    )]);
    let ops = survey_ops(gh, Some(ENGINEER_AUTHOR));
    assert_eq!(
        ops.survey_ready_prs(&[REPO.to_string()]),
        vec![PrRef {
            repo: REPO.to_string(),
            pr: 701,
        }],
        "a labeled engineer PR is a candidate on any branch"
    );
}

/// FAIL-CLOSED default: with no automerge author configured (the
/// `SIMARD_AUTOMERGE_AUTHOR` unset case), the survey yields NO candidates so
/// nothing can ever be merged autonomously by default.
#[test]
fn survey_fail_closed_when_automerge_author_unresolved() {
    let gh = SurveyGh::new(vec![open_pr(
        800,
        ENGINEER_AUTHOR,
        "engineer/800-ffff0000",
        &[crate::overseer::config::SIMARD_ENGINEER_PR_LABEL],
    )]);
    let ops = survey_ops(gh, None); // author unresolved → fail closed
    assert!(
        ops.survey_ready_prs(&[REPO.to_string()]).is_empty(),
        "an unresolved automerge author must fail closed to an empty candidate list"
    );
}

// ───────────────────── draft-PR exclusion (#4339) ────────────────────────────

/// #4339 draft guardrail: a DRAFT PR (`isDraft=true`) — same engineer author,
/// carrying the `simard-autonomous` label, green + mergeable — is EXCLUDED from
/// the survey. A draft can never be merged (`gh pr merge` fails deterministically
/// with "Pull Request is still a draft"), so it must NEVER become a candidate
/// even when author + engineer label + CI + mergeable all pass. Pure narrowing.
#[test]
fn survey_excludes_draft_pr_even_when_all_other_gates_pass() {
    let gh = SurveyGh::new(vec![OpenPrSummary {
        is_draft: Some(true),
        ..open_pr(
            4336,
            ENGINEER_AUTHOR,
            "engineer/4336-draft",
            &[crate::overseer::config::SIMARD_ENGINEER_PR_LABEL],
        )
    }]);
    let ops = survey_ops(gh.clone(), Some(ENGINEER_AUTHOR));
    assert!(
        ops.survey_ready_prs(&[REPO.to_string()]).is_empty(),
        "a draft PR must never be a survey candidate, even when author + engineer \
         label + CI + mergeable all pass"
    );
    assert_eq!(gh.merges(), 0, "the survey rail must never merge a draft");
}

/// #4339 counterpart: the SAME PR as non-draft (`isDraft=false`) IS a candidate.
/// The guardrail is a pure NARROWING — it only removes drafts and must never
/// broaden auto-merge eligibility. Proves the exclusion keys on draft state, not
/// on some incidental field of the draft fixture.
#[test]
fn survey_includes_identical_non_draft_pr() {
    let gh = SurveyGh::new(vec![OpenPrSummary {
        is_draft: Some(false),
        ..open_pr(
            4336,
            ENGINEER_AUTHOR,
            "engineer/4336-draft",
            &[crate::overseer::config::SIMARD_ENGINEER_PR_LABEL],
        )
    }]);
    let ops = survey_ops(gh, Some(ENGINEER_AUTHOR));
    assert_eq!(
        ops.survey_ready_prs(&[REPO.to_string()]),
        vec![PrRef {
            repo: REPO.to_string(),
            pr: 4336,
        }],
        "the same PR that is NOT a draft is an eligible candidate — the draft gate \
         only removes drafts, it never broadens eligibility"
    );
}

/// #4339 fail-closed: when draft state is missing/unknown from the listing
/// (`isDraft` absent ⇒ `None`), the PR is treated as NOT ready and EXCLUDED —
/// mirroring the survey's existing fail-closed posture. Admit ONLY `Some(false)`.
#[test]
fn survey_excludes_pr_with_unknown_draft_state_fail_closed() {
    let gh = SurveyGh::new(vec![OpenPrSummary {
        is_draft: None,
        ..open_pr(
            4336,
            ENGINEER_AUTHOR,
            "engineer/4336-unknown",
            &[crate::overseer::config::SIMARD_ENGINEER_PR_LABEL],
        )
    }]);
    let ops = survey_ops(gh.clone(), Some(ENGINEER_AUTHOR));
    assert!(
        ops.survey_ready_prs(&[REPO.to_string()]).is_empty(),
        "unknown draft state must fail closed to exclusion (admit only isDraft==Some(false))"
    );
    assert_eq!(
        gh.merges(),
        0,
        "the survey rail must never merge on unknown draft state"
    );
}

// ─────────────────────── NotMergeReady is plain-English ──────────────────────

/// The new [`OverseerError::NotMergeReady`] variant must `Display` in plain
/// English — no internal gate jargon leaking to the operator feed.
#[test]
fn not_merge_ready_displays_plain_english() {
    let err = OverseerError::NotMergeReady {
        pr: 4097,
        reason: "the merge-readiness review did not approve this change yet".to_string(),
    };
    let s = err.to_string();
    assert!(s.contains("4097"), "the message names the PR: {s:?}");
    assert!(
        !s.contains("check #7") && !s.contains("DiffReviewer") && !s.contains("fail-closed"),
        "no internal gate jargon may leak: {s:?}"
    );
}

// A tiny compile-time assertion that `VerifyReport` is still the verify() return
// shape (guards against an accidental trait-signature drift during the fix).
#[allow(dead_code)]
fn _verify_returns_verify_report(ops: &MergePrOps) -> Result<VerifyReport, OverseerError> {
    ops.verify(REPO, 1)
}
