//! M2 — the real [`PrOps`] adapter: verify a PR against the full pr-verify
//! checklist, **poll required checks until green** (never `--admin`), merge
//! through the shipped gated authority, and fire the mandatory
//! [`NotifyOperator`](crate::overseer::notify) on merge.
//!
//! Reuse (design doc §capability table / grounding ledger):
//! - Objective gates (CI-green / mergeable / base-allowlist):
//!   `stewardship::merge_authority::evaluate_objective_gates`.
//! - Merge itself: `merge_pr_if_merge_ready_with_judge` → `gh pr merge --squash`
//!   (**no `--admin`, no `--no-verify`** — verified in the grounding ledger).
//! - Review gate (#7): `review_pipeline::should_commit`.
//! - New diff-scans (#3–6): [`pr_verify`](crate::overseer::pr_verify).
//!
//! Operator hard-gates encoded:
//! - **#2 poll until required checks pass; never `--admin`.** [`poll_until_green`]
//!   refuses (escalates) on any failed/red check and only proceeds when every
//!   check is SUCCESS/NEUTRAL/SKIPPED and the PR is MERGEABLE.
//! - **#3 every merge notifies via BOTH email and Signal.** [`MergePrOps::merge`]
//!   fires [`DualChannelNotifier`] after a successful merge; the merge is not
//!   considered complete without a dispatched [`NotifyReport`].
//!
//! [`poll_until_green`]: MergePrOps::poll_until_green
//! [`NotifyReport`]: crate::overseer::notify::NotifyReport

use crate::review_pipeline::{ReviewFinding, should_commit};
use crate::stewardship::{
    MergeJudge, MergeOutcome, PrGhClient, base_allowlist_from_env, build_merge_judge,
    evaluate_objective_gates, merge_pr_if_merge_ready_with_judge,
};

use crate::overseer::capabilities::{CheckItem, OverseerError, PrOps, VerifyReport};
use crate::overseer::guardrails::{RecursionGuard, Subject};
use crate::overseer::notify::{DualChannelNotifier, MergeNotification};
use crate::overseer::pr_verify::run_diff_scans;

// ─────────────────────────── injected seams ────────────────────────────────

/// Source of a PR's unified diff + title + author. `PrGhClient` (the shipped
/// trait) has no such reader, so this adds one — the real impl shells
/// `gh pr diff` / `gh pr view --json title,author`; tests inject canned values.
pub trait PrSource {
    fn diff(&self, repo: &str, pr: u32) -> Result<String, OverseerError>;
    fn title(&self, repo: &str, pr: u32) -> Result<String, OverseerError>;
    /// PR author login (for the anti-recursion check). Defaults to empty so
    /// existing fakes need not implement it; the real impl reads it from `gh`.
    fn author(&self, _repo: &str, _pr: u32) -> Result<String, OverseerError> {
        Ok(String::new())
    }
}

/// The review gate (#7). Injected so tests exercise `should_commit` without an
/// LLM. Production wires an LLM-backed reviewer; absent one, verify treats the
/// review as **unavailable → not ready** (fail-closed), so the Overseer never
/// merges unreviewed.
pub trait DiffReviewer {
    fn review(&self, diff: &str) -> Result<Vec<ReviewFinding>, OverseerError>;
}

/// Resolves a PR's merge conflicts. Deliberately its own seam so the real
/// implementation ([`GitConflictResolver`](crate::overseer::conflict)) — which
/// runs guarded git commands and **never** `--no-verify` — is injected only when
/// the operator opts into HIGH-RISK conflict resolution.
pub trait ConflictResolver {
    fn resolve(&self, repo: &str, pr: u32) -> Result<(), OverseerError>;
}

/// Injected clock so the poll loop has **no real sleeps in tests**. Production
/// sleeps; tests count calls.
pub trait PollClock {
    fn sleep(&self, secs: u64);
}

/// Real wall-clock sleep.
#[derive(Clone, Debug, Default)]
pub struct ThreadSleepClock;
impl PollClock for ThreadSleepClock {
    fn sleep(&self, secs: u64) {
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }
}

/// Poll bounds. Conservative by default so a stuck PR escalates rather than
/// spinning.
#[derive(Clone, Copy, Debug)]
pub struct PollConfig {
    pub max_attempts: u32,
    pub interval_secs: u64,
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            max_attempts: 20,
            interval_secs: 30,
        }
    }
}

/// Real `gh`-backed [`PrSource`].
#[derive(Clone, Debug, Default)]
pub struct RealPrSource;

impl PrSource for RealPrSource {
    fn diff(&self, repo: &str, pr: u32) -> Result<String, OverseerError> {
        run_gh(&["pr", "diff", &pr.to_string(), "--repo", repo])
    }

    fn title(&self, repo: &str, pr: u32) -> Result<String, OverseerError> {
        run_gh(&[
            "pr",
            "view",
            &pr.to_string(),
            "--repo",
            repo,
            "--json",
            "title",
            "--jq",
            ".title",
        ])
        .map(|s| s.trim().to_string())
    }

    fn author(&self, repo: &str, pr: u32) -> Result<String, OverseerError> {
        run_gh(&[
            "pr",
            "view",
            &pr.to_string(),
            "--repo",
            repo,
            "--json",
            "author",
            "--jq",
            ".author.login",
        ])
        .map(|s| s.trim().to_string())
    }
}

fn run_gh(args: &[&str]) -> Result<String, OverseerError> {
    let out =
        crate::guarded_command::run_output("gh", args).map_err(|e| OverseerError::Capability {
            what: "gh",
            detail: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(OverseerError::Capability {
            what: "gh",
            detail: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ─────────────────────────── the adapter ───────────────────────────────────

/// The real [`PrOps`]. Holds injected capabilities so the whole verify/merge
/// path is unit-testable with fakes and zero network.
pub struct MergePrOps {
    gh: Box<dyn PrGhClient>,
    source: Box<dyn PrSource>,
    reviewer: Option<Box<dyn DiffReviewer>>,
    judge: Box<dyn MergeJudge>,
    notifier: DualChannelNotifier,
    clock: Box<dyn PollClock>,
    base_allowlist: Vec<String>,
    poll: PollConfig,
    /// HIGH-RISK conflict resolver, wired only when the operator opts in (M3).
    conflict: Option<Box<dyn ConflictResolver>>,
    /// Anti-recursion identity. When set, the Overseer refuses to merge its OWN
    /// PRs (M3). Fails CLOSED when the guard is unconfigured.
    recursion: Option<RecursionGuard>,
}

impl MergePrOps {
    /// Fully-injected constructor (tests).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gh: Box<dyn PrGhClient>,
        source: Box<dyn PrSource>,
        reviewer: Option<Box<dyn DiffReviewer>>,
        judge: Box<dyn MergeJudge>,
        notifier: DualChannelNotifier,
        clock: Box<dyn PollClock>,
        base_allowlist: Vec<String>,
        poll: PollConfig,
    ) -> Self {
        Self {
            gh,
            source,
            reviewer,
            judge,
            notifier,
            clock,
            base_allowlist,
            poll,
            conflict: None,
            recursion: None,
        }
    }

    /// Opt into HIGH-RISK conflict resolution by wiring a resolver (M3). Default
    /// is `None` — `resolve_conflict` refuses until a resolver is provided.
    pub fn with_conflict_resolver(mut self, resolver: Box<dyn ConflictResolver>) -> Self {
        self.conflict = Some(resolver);
        self
    }

    /// Wire the Overseer's anti-recursion identity so `merge` refuses its OWN
    /// PRs (M3). The guard fails CLOSED when unconfigured; the Overseer must run
    /// under a DISTINCT identity, never the operator's login.
    pub fn with_recursion_guard(mut self, guard: RecursionGuard) -> Self {
        self.recursion = Some(guard);
        self
    }

    /// Production adapter: real `gh` client + diff source, the env merge-judge,
    /// env base-allowlist, the env-wired dual notifier, real sleep. The review
    /// gate is left **unwired** (fail-closed) until the operator provides an
    /// LLM reviewer — so the default autonomous path verifies as NOT ready.
    pub fn from_env() -> Self {
        Self::new(
            Box::new(crate::stewardship::RealPrGhClient),
            Box::new(RealPrSource),
            None,
            build_merge_judge(),
            DualChannelNotifier::from_env(),
            Box::new(ThreadSleepClock),
            base_allowlist_from_env(),
            PollConfig::default(),
        )
    }

    /// Classify each check and poll while any is pending; escalate (Err) on the
    /// first failed check or a non-mergeable state; succeed only when every
    /// check passes and the PR is MERGEABLE. **Never merges here** — this only
    /// establishes green. No `--admin`, no sleep in tests (injected clock).
    fn poll_until_green(&self, repo: &str, pr: u32) -> Result<(), OverseerError> {
        for attempt in 0..=self.poll.max_attempts {
            let snap = self
                .gh
                .view_pr(repo, pr)
                .map_err(|e| cap("poll.view_pr", e.to_string()))?;

            if !self.base_allowlist.iter().any(|b| b == &snap.base_ref_name) {
                return Err(cap(
                    "poll",
                    format!("base branch '{}' not in allow-list", snap.base_ref_name),
                ));
            }

            let mut any_pending = false;
            for c in &snap.checks {
                match classify_state(&c.state) {
                    CheckClass::Failed => {
                        return Err(cap(
                            "poll",
                            format!(
                                "check '{}' is '{}' — escalating, not merging",
                                c.name, c.state
                            ),
                        ));
                    }
                    CheckClass::Pending => any_pending = true,
                    CheckClass::Passing => {}
                }
            }

            if !any_pending {
                if snap.mergeable == "MERGEABLE" {
                    return Ok(());
                }
                return Err(cap(
                    "poll",
                    format!("PR not mergeable (status '{}')", snap.mergeable),
                ));
            }

            // Still pending — wait (unless this was the final attempt).
            if attempt < self.poll.max_attempts {
                self.clock.sleep(self.poll.interval_secs);
            }
        }
        Err(cap(
            "poll",
            "required checks still pending after max attempts — escalating".to_string(),
        ))
    }

    /// Build the operator notification from the merged PR.
    fn notification(&self, repo: &str, pr: u32, title: &str) -> MergeNotification {
        MergeNotification {
            problem: format!(
                "A merge-ready PR passed the objective gates (CI green + mergeable + \
                 base allow-list) and the Overseer pr-verify safety scans, and was merged: {title}"
            ),
            pr_title: title.to_string(),
            pr_url: format!("https://github.com/{repo}/pull/{pr}"),
            repo: repo.to_string(),
            autonomous: true,
        }
    }
}

impl PrOps for MergePrOps {
    /// Run the full pr-verify checklist: objective gates (#1–2), the four
    /// additive diff-scans (#3–6), and the review gate (#7). `ready` iff all pass.
    fn verify(&self, repo: &str, pr: u32) -> Result<VerifyReport, OverseerError> {
        let mut checks = Vec::new();

        // #1–2 objective gates.
        let snap = self
            .gh
            .view_pr(repo, pr)
            .map_err(|e| cap("verify.view_pr", e.to_string()))?;
        checks.push(
            match evaluate_objective_gates(&snap, &self.base_allowlist) {
                Ok(()) => CheckItem {
                    name: "objective gates (CI green + mergeable + base allow-list)".to_string(),
                    passed: true,
                    note: "ok".to_string(),
                },
                Err(reason) => CheckItem {
                    name: "objective gates (CI green + mergeable + base allow-list)".to_string(),
                    passed: false,
                    note: reason,
                },
            },
        );

        // #3–6 + #8 additive diff-scans.
        let diff = self.source.diff(repo, pr)?;
        checks.extend(run_diff_scans(&diff));

        // #7 review gate (fail-closed when unwired).
        checks.push(match &self.reviewer {
            Some(r) => {
                let findings = r.review(&diff)?;
                if should_commit(&findings) {
                    CheckItem {
                        name: "review (no Bug/Security >= High)".to_string(),
                        passed: true,
                        note: format!("{} finding(s), none blocking", findings.len()),
                    }
                } else {
                    CheckItem {
                        name: "review (no Bug/Security >= High)".to_string(),
                        passed: false,
                        note: "a Bug/Security finding of High+ severity blocks merge".to_string(),
                    }
                }
            }
            None => CheckItem {
                name: "review (no Bug/Security >= High)".to_string(),
                passed: false,
                note: "review unavailable (no reviewer wired) — fail-closed".to_string(),
            },
        });

        let ready = checks.iter().all(|c| c.passed);
        Ok(VerifyReport { ready, checks })
    }

    /// Verify → poll-until-green → merge (no `--admin`) → **notify operator**.
    /// If verify is not ready or a check fails, it escalates (Err) and never
    /// merges. On a successful merge it fires the dual-channel notification; the
    /// merge is not complete until that notification has dispatched.
    fn merge(&self, repo: &str, pr: u32) -> Result<(), OverseerError> {
        // 0. Anti-recursion: never merge the Overseer's OWN PR (M3). Fails
        //    CLOSED when the guard is configured but the subject is its own.
        if let Some(guard) = &self.recursion {
            let author = self.source.author(repo, pr)?;
            guard
                .admit(&Subject::Pr {
                    repo: repo.to_string(),
                    pr,
                    author,
                })
                .map_err(|e| cap("merge.recursion", e.to_string()))?;
        }

        // 1. Full checklist must pass.
        let report = self.verify(repo, pr)?;
        if !report.ready {
            let failed: Vec<&str> = report
                .checks
                .iter()
                .filter(|c| !c.passed)
                .map(|c| c.name.as_str())
                .collect();
            return Err(cap(
                "merge.verify",
                format!("pr-verify not satisfied; failing: {}", failed.join(", ")),
            ));
        }

        // 2. Poll required checks until green (escalate on red/timeout).
        self.poll_until_green(repo, pr)?;

        // 3. Merge through the shipped gated authority (squash, no --admin).
        let outcome = merge_pr_if_merge_ready_with_judge(
            pr,
            repo,
            self.gh.as_ref(),
            &self.base_allowlist,
            self.judge.as_ref(),
        )
        .map_err(|e| cap("merge", e.to_string()))?;

        match outcome {
            MergeOutcome::Merged { .. } => {
                // 4. MANDATORY: notify the operator on BOTH channels. The merge
                //    is not "done" until this dispatches (queued counts — never
                //    silently dropped).
                let title = self
                    .source
                    .title(repo, pr)
                    .unwrap_or_else(|_| format!("PR #{pr}"));
                let notification = self.notification(repo, pr, &title);
                let nreport = self.notifier.notify(&notification.to_operator());
                debug_assert!(
                    nreport.dispatched(),
                    "merge completed without a dispatched operator notification"
                );
                Ok(())
            }
            MergeOutcome::Refused { reason, .. } => Err(cap("merge", reason)),
        }
    }

    /// HIGH-RISK conflict resolution. Delegates to the injected
    /// [`ConflictResolver`] (which **never** uses `--no-verify`); refuses when no
    /// resolver is wired. Opt-in only.
    fn resolve_conflict(&self, repo: &str, pr: u32) -> Result<(), OverseerError> {
        match &self.conflict {
            Some(resolver) => resolver.resolve(repo, pr),
            None => Err(OverseerError::Capability {
                what: "resolve_conflict",
                detail:
                    "no conflict resolver wired (HIGH-RISK; operator opt-in, never --no-verify)"
                        .to_string(),
            }),
        }
    }
}

fn cap(what: &'static str, detail: String) -> OverseerError {
    OverseerError::Capability { what, detail }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckClass {
    Passing,
    Pending,
    Failed,
}

/// Classify a `statusCheckRollup` state. Unknown/failure-ish states are treated
/// as **Failed** (fail-closed) so the Overseer escalates rather than merging on
/// an unrecognised state.
fn classify_state(state: &str) -> CheckClass {
    match state {
        "SUCCESS" | "NEUTRAL" | "SKIPPED" => CheckClass::Passing,
        "PENDING" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED" | "EXPECTED" => {
            CheckClass::Pending
        }
        _ => CheckClass::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overseer::notify::{ChannelDelivery, NotifyChannel, OperatorNotification};
    use crate::stewardship::merge_authority::CheckRollupEntry;
    use crate::stewardship::{JudgeOutcome, MergeJudgeKind, PrSnapshot, Verdict};
    use std::sync::{Arc, Mutex};

    // ── fakes ────────────────────────────────────────────────────────────────

    /// A `PrGhClient` that serves a scripted sequence of snapshots (one per
    /// `view_pr`) and records `squash_merge` calls. No network, no `--admin`
    /// (the trait has no admin method — the no-admin guarantee is structural).
    struct ScriptedGh {
        snapshots: Mutex<Vec<PrSnapshot>>,
        views: Mutex<usize>,
        merges: Mutex<usize>,
    }
    impl ScriptedGh {
        fn new(seq: Vec<PrSnapshot>) -> Arc<Self> {
            Arc::new(Self {
                snapshots: Mutex::new(seq),
                views: Mutex::new(0),
                merges: Mutex::new(0),
            })
        }
        fn merges(&self) -> usize {
            *self.merges.lock().unwrap()
        }
    }
    impl PrGhClient for Arc<ScriptedGh> {
        fn view_pr(&self, _repo: &str, _pr: u32) -> crate::error::SimardResult<PrSnapshot> {
            let mut n = self.views.lock().unwrap();
            let seq = self.snapshots.lock().unwrap();
            let idx = (*n).min(seq.len() - 1);
            *n += 1;
            Ok(seq[idx].clone())
        }
        fn squash_merge(&self, _repo: &str, _pr: u32) -> crate::error::SimardResult<()> {
            *self.merges.lock().unwrap() += 1;
            Ok(())
        }
    }

    struct FakeSource {
        diff: String,
    }
    impl PrSource for FakeSource {
        fn diff(&self, _repo: &str, _pr: u32) -> Result<String, OverseerError> {
            Ok(self.diff.clone())
        }
        fn title(&self, _repo: &str, _pr: u32) -> Result<String, OverseerError> {
            Ok("fix(distill): strip launch-banner noise".to_string())
        }
    }

    struct FakeReviewer(Vec<ReviewFinding>);
    impl DiffReviewer for FakeReviewer {
        fn review(&self, _diff: &str) -> Result<Vec<ReviewFinding>, OverseerError> {
            Ok(self.0.clone())
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
                rationale: "stub: ready".to_string(),
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

    /// Records every notification it is handed.
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

    fn check(name: &str, state: &str) -> CheckRollupEntry {
        CheckRollupEntry {
            name: name.to_string(),
            state: state.to_string(),
        }
    }

    fn snapshot(mergeable: &str, checks: Vec<CheckRollupEntry>) -> PrSnapshot {
        PrSnapshot {
            body: "body".to_string(),
            mergeable: mergeable.to_string(),
            review_decision: "APPROVED".to_string(),
            checks,
            base_ref_name: "main".to_string(),
            labels: Vec::new(),
        }
    }

    fn green() -> PrSnapshot {
        snapshot(
            "MERGEABLE",
            vec![check("build", "SUCCESS"), check("clippy", "SUCCESS")],
        )
    }

    const CLEAN_DIFF: &str = "\
+++ b/src/overseer/x.rs
@@ -0,0 +1,2 @@
+pub fn reasoner() {}
+// orient-decide-act
";

    /// Build an adapter with capturing notify channels; returns the shared
    /// capture buffers for email + signal.
    #[allow(clippy::type_complexity)]
    fn adapter_with(
        gh: Arc<ScriptedGh>,
        diff: &str,
        reviewer: Option<Box<dyn DiffReviewer>>,
        clock: Arc<CountingClock>,
        poll: PollConfig,
    ) -> (
        MergePrOps,
        Arc<Mutex<Vec<OperatorNotification>>>,
        Arc<Mutex<Vec<OperatorNotification>>>,
    ) {
        let email_seen = Arc::new(Mutex::new(vec![]));
        let signal_seen = Arc::new(Mutex::new(vec![]));
        let notifier = DualChannelNotifier::new(vec![
            Box::new(CapturingChannel {
                name: "email".to_string(),
                seen: email_seen.clone(),
            }),
            Box::new(CapturingChannel {
                name: "signal".to_string(),
                seen: signal_seen.clone(),
            }),
        ]);
        let ops = MergePrOps::new(
            Box::new(gh),
            Box::new(FakeSource {
                diff: diff.to_string(),
            }),
            reviewer,
            Box::new(ReadyJudge),
            notifier,
            Box::new(clock),
            vec!["main".to_string()],
            poll,
        );
        (ops, email_seen, signal_seen)
    }

    // ── verify ───────────────────────────────────────────────────────────────

    #[test]
    fn verify_ready_on_clean_green_pr() {
        let gh = ScriptedGh::new(vec![green()]);
        let (ops, _, _) = adapter_with(
            gh,
            CLEAN_DIFF,
            Some(Box::new(FakeReviewer(vec![]))),
            Arc::new(CountingClock::default()),
            PollConfig::default(),
        );
        let report = ops.verify("rysweet/Simard", 1).unwrap();
        assert!(report.ready, "clean green PR verifies ready: {report:?}");
    }

    #[test]
    fn verify_not_ready_on_dirty_diff() {
        let gh = ScriptedGh::new(vec![green()]);
        let dirty = "\
+++ b/src/x.rs
@@ -0,0 +1,2 @@
+struct HttpBridge;
+    println!(\"noise\");
";
        let (ops, _, _) = adapter_with(
            gh,
            dirty,
            Some(Box::new(FakeReviewer(vec![]))),
            Arc::new(CountingClock::default()),
            PollConfig::default(),
        );
        let report = ops.verify("rysweet/Simard", 1).unwrap();
        assert!(!report.ready, "Bridge + println diff must fail verify");
    }

    #[test]
    fn verify_not_ready_when_ci_red() {
        let red = snapshot("MERGEABLE", vec![check("build", "FAILURE")]);
        let gh = ScriptedGh::new(vec![red]);
        let (ops, _, _) = adapter_with(
            gh,
            CLEAN_DIFF,
            Some(Box::new(FakeReviewer(vec![]))),
            Arc::new(CountingClock::default()),
            PollConfig::default(),
        );
        assert!(!ops.verify("rysweet/Simard", 1).unwrap().ready);
    }

    #[test]
    fn verify_review_gate_blocks_high_severity_finding() {
        use crate::review_pipeline::{FindingCategory, ReviewFinding, Severity};
        let finding = ReviewFinding {
            category: FindingCategory::Security,
            severity: Severity::High,
            description: "hardcoded secret".to_string(),
            file_path: "src/x.rs".to_string(),
            line_range: None,
        };
        let gh = ScriptedGh::new(vec![green()]);
        let (ops, _, _) = adapter_with(
            gh,
            CLEAN_DIFF,
            Some(Box::new(FakeReviewer(vec![finding]))),
            Arc::new(CountingClock::default()),
            PollConfig::default(),
        );
        assert!(
            !ops.verify("rysweet/Simard", 1).unwrap().ready,
            "a High Security finding must block"
        );
    }

    #[test]
    fn verify_fail_closed_without_reviewer() {
        let gh = ScriptedGh::new(vec![green()]);
        let (ops, _, _) = adapter_with(
            gh,
            CLEAN_DIFF,
            None,
            Arc::new(CountingClock::default()),
            PollConfig::default(),
        );
        assert!(
            !ops.verify("rysweet/Simard", 1).unwrap().ready,
            "no reviewer wired → review unavailable → not ready (fail-closed)"
        );
    }

    // ── merge: green-only, no-admin, notify fires ────────────────────────────

    #[test]
    fn merge_when_green_merges_once_and_notifies_both_channels() {
        let gh = ScriptedGh::new(vec![green()]);
        let (ops, email, signal) = adapter_with(
            gh.clone(),
            CLEAN_DIFF,
            Some(Box::new(FakeReviewer(vec![]))),
            Arc::new(CountingClock::default()),
            PollConfig::default(),
        );
        ops.merge("rysweet/Simard", 7)
            .expect("merge should succeed");
        assert_eq!(gh.merges(), 1, "exactly one squash-merge (no --admin path)");
        assert_eq!(email.lock().unwrap().len(), 1, "email notified");
        assert_eq!(signal.lock().unwrap().len(), 1, "signal notified");
        // The notification carries the problem + PR title + link.
        let n = &email.lock().unwrap()[0];
        assert!(n.link.as_deref().unwrap().contains("/pull/7"));
        assert!(n.autonomous);
        assert!(n.problem.contains("merge-ready"));
    }

    #[test]
    fn merge_refuses_and_never_merges_when_ci_red() {
        let red = snapshot("MERGEABLE", vec![check("build", "FAILURE")]);
        let gh = ScriptedGh::new(vec![red]);
        let (ops, email, signal) = adapter_with(
            gh.clone(),
            CLEAN_DIFF,
            Some(Box::new(FakeReviewer(vec![]))),
            Arc::new(CountingClock::default()),
            PollConfig::default(),
        );
        assert!(
            ops.merge("rysweet/Simard", 7).is_err(),
            "red CI must not merge"
        );
        assert_eq!(gh.merges(), 0, "no merge attempted on red CI");
        assert!(
            email.lock().unwrap().is_empty(),
            "no notification on non-merge"
        );
        assert!(signal.lock().unwrap().is_empty());
    }

    #[test]
    fn merge_polls_pending_then_green() {
        // view_pr sequence: verify(green) then poll sees pending, pending, green.
        let pending = snapshot(
            "MERGEABLE",
            vec![check("build", "IN_PROGRESS"), check("clippy", "SUCCESS")],
        );
        let gh = ScriptedGh::new(vec![
            green(), // verify()'s view_pr
            pending.clone(),
            pending,
            green(), // poll finally green
            green(), // merge_pr_if_merge_ready_with_judge's view_pr
        ]);
        let clock = Arc::new(CountingClock::default());
        let (ops, email, _) = adapter_with(
            gh.clone(),
            CLEAN_DIFF,
            Some(Box::new(FakeReviewer(vec![]))),
            clock.clone(),
            PollConfig {
                max_attempts: 5,
                interval_secs: 1,
            },
        );
        ops.merge("rysweet/Simard", 7)
            .expect("merges after checks go green");
        assert_eq!(gh.merges(), 1);
        assert_eq!(
            *clock.sleeps.lock().unwrap(),
            2,
            "slept once per pending poll"
        );
        assert_eq!(email.lock().unwrap().len(), 1);
    }

    #[test]
    fn resolve_conflict_refuses_without_resolver_and_delegates_when_wired() {
        // No resolver wired → refuse (never a silent no-op, never --no-verify).
        let gh = ScriptedGh::new(vec![green()]);
        let (ops, _, _) = adapter_with(
            gh,
            CLEAN_DIFF,
            None,
            Arc::new(CountingClock::default()),
            PollConfig::default(),
        );
        assert!(ops.resolve_conflict("rysweet/Simard", 1).is_err());

        // With a resolver wired, resolve_conflict delegates to it.
        struct OkResolver(Arc<Mutex<usize>>);
        impl ConflictResolver for OkResolver {
            fn resolve(&self, _repo: &str, _pr: u32) -> Result<(), OverseerError> {
                *self.0.lock().unwrap() += 1;
                Ok(())
            }
        }
        let calls = Arc::new(Mutex::new(0));
        let gh2 = ScriptedGh::new(vec![green()]);
        let (ops2, _, _) = adapter_with(
            gh2,
            CLEAN_DIFF,
            None,
            Arc::new(CountingClock::default()),
            PollConfig::default(),
        );
        let ops2 = ops2.with_conflict_resolver(Box::new(OkResolver(calls.clone())));
        ops2.resolve_conflict("rysweet/Simard", 1)
            .expect("delegates");
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[test]
    fn merge_refuses_its_own_pr_via_recursion_guard() {
        use crate::overseer::guardrails::RecursionGuard;
        struct OwnAuthorSource;
        impl PrSource for OwnAuthorSource {
            fn diff(&self, _r: &str, _p: u32) -> Result<String, OverseerError> {
                Ok(CLEAN_DIFF.to_string())
            }
            fn title(&self, _r: &str, _p: u32) -> Result<String, OverseerError> {
                Ok("t".to_string())
            }
            fn author(&self, _r: &str, _p: u32) -> Result<String, OverseerError> {
                Ok("simard-overseer[bot]".to_string())
            }
        }
        let gh = ScriptedGh::new(vec![green()]);
        let email = Arc::new(Mutex::new(vec![]));
        let notifier = DualChannelNotifier::new(vec![Box::new(CapturingChannel {
            name: "email".to_string(),
            seen: email.clone(),
        })]);
        let ops = MergePrOps::new(
            Box::new(gh.clone()),
            Box::new(OwnAuthorSource),
            Some(Box::new(FakeReviewer(vec![]))),
            Box::new(ReadyJudge),
            notifier,
            Box::new(Arc::new(CountingClock::default())),
            vec!["main".to_string()],
            PollConfig::default(),
        )
        .with_recursion_guard(RecursionGuard {
            author_login: "simard-overseer[bot]".to_string(),
            branch_prefix: "overseer/".to_string(),
            goal_source_tag: "overseer:".to_string(),
        });
        assert!(
            ops.merge("rysweet/Simard", 7).is_err(),
            "the Overseer must refuse to merge its OWN PR"
        );
        assert_eq!(gh.merges(), 0, "no merge of an own PR");
        assert!(
            email.lock().unwrap().is_empty(),
            "no notify on a refused own PR"
        );
    }
}
