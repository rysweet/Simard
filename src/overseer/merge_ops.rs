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
//! - Review authority: the ALREADY-WIRED agentic `MergeJudge`
//!   (`prompt_assets/…/merge_readiness_judge.md`), the SINGLE source of
//!   merge-readiness review truth, invoked at `merge()` step 3. `verify()` is a
//!   review-FREE objective pre-filter (#4097).
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

use crate::stewardship::{
    MergeJudge, MergeOutcome, PrGhClient, base_allowlist_from_env, build_merge_judge,
    evaluate_objective_gates, merge_pr_if_merge_ready_with_judge,
};

use crate::overseer::capabilities::{CheckItem, OverseerError, PrOps, PrRef, VerifyReport};
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
    let out = std::process::Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| OverseerError::Capability {
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
    /// The OODA/engineer `gh` login Simard authors her PRs under (#4097). The
    /// `survey_ready_prs` sensor lists ONLY PRs whose author EXACTLY matches, so
    /// it never surveys a human's PR. `None` => fail-closed (no candidates).
    automerge_author: Option<String>,
}

impl MergePrOps {
    /// Fully-injected constructor (tests).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gh: Box<dyn PrGhClient>,
        source: Box<dyn PrSource>,
        judge: Box<dyn MergeJudge>,
        notifier: DualChannelNotifier,
        clock: Box<dyn PollClock>,
        base_allowlist: Vec<String>,
        poll: PollConfig,
    ) -> Self {
        Self {
            gh,
            source,
            judge,
            notifier,
            clock,
            base_allowlist,
            poll,
            conflict: None,
            recursion: None,
            automerge_author: None,
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

    /// Wire the OODA/engineer author login the autonomous-self-merge sensor
    /// (#4097) filters candidates to. Without it, `survey_ready_prs` cannot tell
    /// Simard's OWN PRs from a human's and yields NO candidates (fail-closed).
    pub fn with_automerge_author(mut self, author: String) -> Self {
        self.automerge_author = Some(author);
        self
    }

    /// Production adapter: real `gh` client + diff source, the env merge-judge,
    /// env base-allowlist, the env-wired dual notifier, real sleep. `verify()` is
    /// a review-FREE objective pre-filter; the SINGLE review authority is the
    /// agentic `MergeJudge` (`merge_readiness_judge.md`) invoked at `merge()`
    /// step 3. When no LLM provider is configured, [`build_merge_judge`] returns
    /// the fail-closed `RefusingMergeJudge`, so the default autonomous path
    /// escalates rather than merging unreviewed.
    pub fn from_env() -> Self {
        let mut ops = Self::new(
            Box::new(crate::stewardship::RealPrGhClient),
            Box::new(RealPrSource),
            build_merge_judge(),
            DualChannelNotifier::from_env(),
            Box::new(ThreadSleepClock),
            base_allowlist_from_env(),
            PollConfig::default(),
        );
        // Wire the OODA/engineer author so the self-merge sensor (#4097) can tell
        // Simard's own PRs from a human's. `None` (default) => fail-closed.
        ops.automerge_author = crate::overseer::config::automerge_author();
        ops
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

    /// Build the operator notification from the merged PR. The `problem` text is
    /// deliberately PLAIN ENGLISH — it names what was solved (the PR title) and
    /// that it cleared every check and review, with NO internal gate jargon
    /// ("objective gates", "pr-verify scans", "base allow-list", "MergeJudge").
    /// The PR link rides `pr_url` so the operator can open the merged PR from
    /// their phone. See `docs/reference/overseer-operator-notifications.md`.
    fn notification(&self, repo: &str, pr: u32, title: &str) -> MergeNotification {
        MergeNotification {
            problem: format!(
                "Merged: \"{title}\". It passed every check and review, so Simard \
                 merged it for you."
            ),
            pr_title: title.to_string(),
            pr_url: format!("https://github.com/{repo}/pull/{pr}"),
            repo: repo.to_string(),
            autonomous: true,
        }
    }
}

impl PrOps for MergePrOps {
    /// Run the OBJECTIVE pre-filter: objective gates (#1–2) and the additive
    /// deterministic diff-scans (#3–6/#8). `ready` iff all pass. This carries NO
    /// review gate — the agentic `MergeJudge` (`merge_readiness_judge.md`) is the
    /// SOLE review authority and runs downstream in [`merge`](Self::merge)
    /// step 3. `ready == true` therefore means "eligible to proceed to the
    /// authoritative merge", not "approved to merge".
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

        let ready = checks.iter().all(|c| c.passed);
        Ok(VerifyReport { ready, checks })
    }

    /// Verify → poll-until-green → merge (no `--admin`) → **notify operator**.
    /// If verify is not ready or a check fails, it escalates (Err) and never
    /// merges. On a successful merge it fires the dual-channel notification; the
    /// merge is not complete until that notification has dispatched.
    fn merge(&self, repo: &str, pr: u32) -> Result<(), OverseerError> {
        // 0. Anti-recursion + author re-assert. Both fail CLOSED; the PR author
        //    is fetched once and shared by both gates.
        //    (a) Never merge the Overseer's OWN PR (M3).
        //    (b) When an autonomous-merge identity is configured, independently
        //        re-verify at THIS authoritative step that the PR is really
        //        authored by that identity. The survey filter is the sole
        //        production feeder of `ready_prs` today, so this is pure
        //        defense-in-depth: it keeps the author gate robust against any
        //        FUTURE path that could enqueue a `PrRef` from another source
        //        (per the Step 17c security review's hardening note). The match
        //        is case-INSENSITIVE, consistent with the survey filter; an
        //        empty/mismatched author can never match and is refused.
        if self.recursion.is_some() || self.automerge_author.is_some() {
            let author = self.source.author(repo, pr)?;
            if let Some(guard) = &self.recursion {
                guard
                    .admit(&Subject::Pr {
                        repo: repo.to_string(),
                        pr,
                        author: author.clone(),
                    })
                    .map_err(|e| cap("merge.recursion", e.to_string()))?;
            }
            if let Some(expected) = self.automerge_author.as_deref()
                && !author.eq_ignore_ascii_case(expected)
            {
                return Err(cap(
                    "merge.author",
                    format!(
                        "refusing to merge {repo}#{pr}: author {author:?} does not \
                         match the configured autonomous-merge identity \
                         (fail-closed)"
                    ),
                ));
            }
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
            MergeOutcome::Refused { pr_number, reason } => {
                // The authoritative agentic review (or a fail-closed judge on a
                // provider outage) did not approve this PR. Surface it as
                // NotMergeReady so the Act handler ESCALATES to the operator — it
                // is never merged blindly and never a hard error.
                Err(OverseerError::NotMergeReady {
                    pr: pr_number,
                    reason,
                })
            }
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

    /// The thin deterministic self-merge sensor rail (#4097). For each
    /// allowlisted `owner/name` repo it lists Simard's OWN open PRs (ONE
    /// author-scoped `gh pr list` per repo, reusing
    /// [`PrGhClient::list_prs_by_author`] so the author filter runs SERVER-SIDE
    /// — a busy repo can never crowd Simard's PRs out of the fetch window, and
    /// only her PRs are transferred + parsed) and keeps only those that
    /// are (a) authored by the configured automerge author (re-verified with a
    /// case-insensitive whole-login in-process match as defense-in-depth),
    /// (b) prove Simard-origin — the G3 engineer-PR gate: they carry the durable
    /// [`SIMARD_ENGINEER_PR_LABEL`] (primary) OR ride a Rust-deterministic
    /// engineer-only branch namespace ([`is_engineer_branch`]: secondary), which
    /// is what separates Simard's OWN engineer PRs from the operator's own review
    /// PRs when both share the same author login — and (c) pass the cheap
    /// objective pre-filter
    /// ([`evaluate_objective_gates`]: base allow-list + `mergeable == MERGEABLE`
    /// + all checks green) computed from the already-fetched listing fields.
    ///
    /// This is a candidate LIST only: it NEVER calls `view_pr`, NEVER runs the
    /// MergeJudge, and NEVER merges. The authoritative six-criteria gate stays
    /// downstream in `merge_authority` and remains the single source of merge
    /// truth. Every failure mode is fail-closed AND fail-visible:
    /// - no automerge author configured => empty (logged) — cannot distinguish own PRs;
    /// - a PR that is neither labeled nor on an engineer branch => excluded (never a candidate);
    /// - a `gh pr list` error for a repo => that repo is skipped (logged), others still surveyed;
    /// - an empty allowlist => empty, and `gh` is never even called.
    ///
    /// [`SIMARD_ENGINEER_PR_LABEL`]: crate::overseer::config::SIMARD_ENGINEER_PR_LABEL
    /// [`is_engineer_branch`]: crate::overseer::config::is_engineer_branch
    fn survey_ready_prs(&self, repos: &[String]) -> Vec<PrRef> {
        let Some(author) = self.automerge_author.as_deref() else {
            if !repos.is_empty() {
                tracing::warn!(
                    target: "overseer::merge",
                    "autonomous-self-merge sensor: SIMARD_AUTOMERGE_AUTHOR unset — \
                     cannot distinguish Simard's own PRs from a human's; yielding no \
                     candidates (fail-closed)"
                );
            }
            return Vec::new();
        };

        let mut candidates = Vec::new();
        for repo in repos {
            // A generous per-repo cap: the survey is O(open PRs), and the
            // downstream gate re-verifies each candidate authoritatively. The
            // author filter is pushed SERVER-SIDE so a repo with >100 open PRs
            // can never crowd Simard's own eligible PRs out of this window.
            let summaries = match self.gh.list_prs_by_author(repo, author, 100) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        target: "overseer::merge",
                        repo = %repo,
                        error = %e,
                        "autonomous-self-merge sensor: `gh pr list` failed — skipping this \
                         repo (fail-visible); other repos still surveyed"
                    );
                    continue;
                }
            };
            for pr in summaries {
                // Case-INSENSITIVE whole-login match: GitHub logins are unique
                // case-insensitively and `gh pr list --author` already matches
                // that way server-side, so a byte-exact compare here would
                // silently drop every returned row when the operator configured
                // `SIMARD_AUTOMERGE_AUTHOR` with different casing than the
                // canonical `author.login` — a "canary does nothing" trap. This
                // stays a WHOLE-login equality (a substring/prefix would let a
                // look-alike or human PR through). An empty author (missing
                // object) can never match, so it still fails closed.
                if !pr.author.eq_ignore_ascii_case(author) {
                    continue;
                }
                // G3 engineer-PR gate (#4097 safe-enablement). The author filter
                // (G2) alone is INSUFFICIENT: Simard's engineer PRs AND the
                // operator's OWN review PRs are authored by the same `gh` login
                // (e.g. `rysweet`), so G2 would make the operator's review PRs
                // (e.g. #3142 `cogthreads/…`) eligible to auto-merge — which is
                // unacceptable. Narrow to PRs that PROVE Simard-origin: they
                // carry the durable `simard-autonomous` label (PRIMARY, works on
                // shared `feat/`/`fix/` prefixes) OR ride a Rust-deterministic
                // engineer-only branch namespace (SECONDARY, defense-in-depth for
                // when the best-effort label was not applied). A PR with NEITHER
                // marker is NEVER a candidate, even when author + CI + mergeable
                // all pass. This is a pure NARROWING — it can only remove.
                let is_engineer_pr = pr
                    .labels
                    .iter()
                    .any(|l| crate::overseer::config::is_engineer_pr_label(l))
                    || crate::overseer::config::is_engineer_branch(&pr.head_ref_name);
                if !is_engineer_pr {
                    continue;
                }
                // Cheap objective pre-filter from the ALREADY-FETCHED listing
                // fields — no extra `gh` read. The full MergeJudge runs later.
                if evaluate_objective_gates(&pr.to_snapshot(), &self.base_allowlist).is_ok() {
                    candidates.push(PrRef {
                        repo: repo.clone(),
                        pr: pr.number,
                    });
                }
            }
        }
        candidates
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
            Arc::new(CountingClock::default()),
            PollConfig::default(),
        );
        assert!(!ops.verify("rysweet/Simard", 1).unwrap().ready);
    }

    // ── merge: green-only, no-admin, notify fires ────────────────────────────

    #[test]
    fn merge_when_green_merges_once_and_notifies_both_channels() {
        let gh = ScriptedGh::new(vec![green()]);
        let (ops, email, signal) = adapter_with(
            gh.clone(),
            CLEAN_DIFF,
            Arc::new(CountingClock::default()),
            PollConfig::default(),
        );
        ops.merge("rysweet/Simard", 7)
            .expect("merge should succeed");
        assert_eq!(gh.merges(), 1, "exactly one squash-merge (no --admin path)");
        assert_eq!(email.lock().unwrap().len(), 1, "email notified");
        assert_eq!(signal.lock().unwrap().len(), 1, "signal notified");
        // The notification carries the plain-English problem + PR title + link,
        // with no internal gate jargon leaking to the operator (R5).
        let n = &email.lock().unwrap()[0];
        assert!(n.link.as_deref().unwrap().contains("/pull/7"));
        assert!(n.autonomous);
        assert!(
            n.problem.contains("passed every check and review"),
            "the notification must explain in plain English what happened"
        );
        assert!(
            !n.problem.contains("objective gates")
                && !n.problem.contains("pr-verify")
                && !n.problem.contains("allow-list"),
            "the operator notification must not surface internal gate jargon"
        );
    }

    #[test]
    fn merge_refuses_and_never_merges_when_ci_red() {
        let red = snapshot("MERGEABLE", vec![check("build", "FAILURE")]);
        let gh = ScriptedGh::new(vec![red]);
        let (ops, email, signal) = adapter_with(
            gh.clone(),
            CLEAN_DIFF,
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

    #[test]
    fn merge_refuses_when_author_mismatches_configured_automerge_identity() {
        // Defense-in-depth (Step 17c hardening): even on a fully GREEN,
        // mergeable, reviewed PR, the authoritative merge step must refuse a PR
        // whose author is NOT the configured autonomous-merge identity — proving
        // the author gate no longer rests solely on the upstream survey filter.
        struct HumanAuthorSource;
        impl PrSource for HumanAuthorSource {
            fn diff(&self, _r: &str, _p: u32) -> Result<String, OverseerError> {
                Ok(CLEAN_DIFF.to_string())
            }
            fn title(&self, _r: &str, _p: u32) -> Result<String, OverseerError> {
                Ok("t".to_string())
            }
            fn author(&self, _r: &str, _p: u32) -> Result<String, OverseerError> {
                Ok("human-dev".to_string())
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
            Box::new(HumanAuthorSource),
            Box::new(ReadyJudge),
            notifier,
            Box::new(Arc::new(CountingClock::default())),
            vec!["main".to_string()],
            PollConfig::default(),
        )
        .with_automerge_author("simard-engineer".to_string());
        assert!(
            ops.merge("rysweet/Simard", 7).is_err(),
            "a non-automerge-author PR must be refused at the merge step (fail-closed)"
        );
        assert_eq!(gh.merges(), 0, "no merge of a foreign-author PR");
        assert!(
            email.lock().unwrap().is_empty(),
            "no notify on a refused foreign-author PR"
        );
    }

    #[test]
    fn merge_allows_configured_automerge_author_case_insensitively() {
        // The merge-step author re-assert uses the SAME case-insensitive whole-
        // login comparison as the survey filter, so a casing difference between
        // `SIMARD_AUTOMERGE_AUTHOR` and the canonical `author.login` still merges
        // (no "canary does nothing" trap) rather than silently refusing.
        struct MatchingAuthorSource;
        impl PrSource for MatchingAuthorSource {
            fn diff(&self, _r: &str, _p: u32) -> Result<String, OverseerError> {
                Ok(CLEAN_DIFF.to_string())
            }
            fn title(&self, _r: &str, _p: u32) -> Result<String, OverseerError> {
                Ok("t".to_string())
            }
            fn author(&self, _r: &str, _p: u32) -> Result<String, OverseerError> {
                Ok("Simard-Engineer".to_string())
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
            Box::new(MatchingAuthorSource),
            Box::new(ReadyJudge),
            notifier,
            Box::new(Arc::new(CountingClock::default())),
            vec!["main".to_string()],
            PollConfig::default(),
        )
        .with_automerge_author("simard-engineer".to_string());
        ops.merge("rysweet/Simard", 7)
            .expect("matching automerge author (case-insensitive) should merge");
        assert_eq!(
            gh.merges(),
            1,
            "exactly one squash-merge for the matched author"
        );
        assert_eq!(email.lock().unwrap().len(), 1, "operator notified on merge");
    }

    // ── survey_ready_prs: the dead-wire sensor rail (issue #4097) ────────────
    //
    // These tests pin the THIN DETERMINISTIC candidate-listing rail that
    // populates `ObservedState.ready_prs`. The rail only LISTS candidates
    // (author-filter + the cheap objective pre-filter on already-fetched
    // statusCheckRollup/mergeable); the authoritative six-criteria gate stays
    // downstream in merge_authority and remains the single source of merge
    // truth. The sensor must NEVER merge and must fail CLOSED + VISIBLE.

    use std::collections::HashMap;

    /// A `PrGhClient` whose `list_open_prs` is scripted per-repo (an `Ok` list
    /// or a simulated `gh` failure). Records `squash_merge` calls so a test can
    /// assert the SENSOR never merges. `view_pr` is deliberately `unreachable!`
    /// — the survey rail must decide from the listing fields alone, never by
    /// re-reading each PR (that is the downstream verify/merge path's job).
    #[allow(clippy::type_complexity)]
    struct ListingGh {
        by_repo:
            HashMap<String, Result<Vec<crate::stewardship::merge_authority::OpenPrSummary>, ()>>,
        listed: Mutex<Vec<String>>,
        authors: Mutex<Vec<String>>,
        merges: Mutex<usize>,
    }
    impl ListingGh {
        fn new(
            by_repo: HashMap<
                String,
                Result<Vec<crate::stewardship::merge_authority::OpenPrSummary>, ()>,
            >,
        ) -> Arc<Self> {
            Arc::new(Self {
                by_repo,
                listed: Mutex::new(Vec::new()),
                authors: Mutex::new(Vec::new()),
                merges: Mutex::new(0),
            })
        }
        fn listed(&self) -> Vec<String> {
            self.listed.lock().unwrap().clone()
        }
        fn authors(&self) -> Vec<String> {
            self.authors.lock().unwrap().clone()
        }
        fn merges(&self) -> usize {
            *self.merges.lock().unwrap()
        }
    }
    impl PrGhClient for Arc<ListingGh> {
        fn view_pr(&self, _repo: &str, _pr: u32) -> crate::error::SimardResult<PrSnapshot> {
            unreachable!(
                "the survey rail must not call view_pr — it lists from listing fields only"
            );
        }
        fn squash_merge(&self, _repo: &str, _pr: u32) -> crate::error::SimardResult<()> {
            *self.merges.lock().unwrap() += 1;
            Ok(())
        }
        fn list_open_prs(
            &self,
            repo: &str,
            _limit: u32,
        ) -> crate::error::SimardResult<Vec<crate::stewardship::merge_authority::OpenPrSummary>>
        {
            self.listed.lock().unwrap().push(repo.to_string());
            match self.by_repo.get(repo) {
                Some(Ok(v)) => Ok(v.clone()),
                Some(Err(())) => Err(crate::error::SimardError::MergeAuthorityGhCommandFailed {
                    reason: "simulated gh pr list failure".to_string(),
                }),
                None => Ok(Vec::new()),
            }
        }
        fn list_prs_by_author(
            &self,
            repo: &str,
            author: &str,
            limit: u32,
        ) -> crate::error::SimardResult<Vec<crate::stewardship::merge_authority::OpenPrSummary>>
        {
            // Record the author the survey pushes down (the server-side filter),
            // then reuse the scripted unscoped listing so existing per-repo
            // scenarios stay valid.
            self.authors.lock().unwrap().push(author.to_string());
            self.list_open_prs(repo, limit)
        }
    }

    fn open_pr(
        number: u32,
        author: &str,
        mergeable: &str,
        base: &str,
        check_state: &str,
    ) -> crate::stewardship::merge_authority::OpenPrSummary {
        // Default helper builds an ENGINEER-identified PR (carries the durable
        // `simard-autonomous` label) so the pre-#4097 candidate expectations —
        // which predate the G3 engineer-PR gate — stay valid. Operator-shaped
        // PRs (no label, non-engineer branch) are built with `open_pr_full`.
        open_pr_full(
            number,
            author,
            mergeable,
            base,
            check_state,
            &format!("feat/{number}"),
            &[crate::overseer::config::SIMARD_ENGINEER_PR_LABEL],
        )
    }

    /// Full control over the head branch + labels so a test can build an
    /// operator-shaped PR (no engineer label, non-engineer branch) or an
    /// engineer PR identified solely by its branch namespace.
    fn open_pr_full(
        number: u32,
        author: &str,
        mergeable: &str,
        base: &str,
        check_state: &str,
        head: &str,
        labels: &[&str],
    ) -> crate::stewardship::merge_authority::OpenPrSummary {
        crate::stewardship::merge_authority::OpenPrSummary {
            number,
            title: format!("candidate PR #{number}"),
            head_ref_name: head.to_string(),
            base_ref_name: base.to_string(),
            mergeable: mergeable.to_string(),
            checks: vec![check("ci", check_state)],
            url: format!("https://github.com/rysweet/Simard/pull/{number}"),
            author: author.to_string(),
            labels: labels.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn pr_ref(repo: &str, pr: u32) -> crate::overseer::capabilities::PrRef {
        crate::overseer::capabilities::PrRef {
            repo: repo.to_string(),
            pr,
        }
    }

    /// Build a `MergePrOps` wired to the listing fake, base-allowlist `[main]`,
    /// and an optional automerge author. Returns the captured notify buffer so
    /// a test can assert the sensor stays SILENT.
    fn survey_ops(
        gh: Arc<ListingGh>,
        author: Option<&str>,
    ) -> (MergePrOps, Arc<Mutex<Vec<OperatorNotification>>>) {
        let seen = Arc::new(Mutex::new(vec![]));
        let notifier = DualChannelNotifier::new(vec![Box::new(CapturingChannel {
            name: "email".to_string(),
            seen: seen.clone(),
        })]);
        let mut ops = MergePrOps::new(
            Box::new(gh),
            Box::new(FakeSource {
                diff: String::new(),
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
        (ops, seen)
    }

    #[test]
    fn survey_selects_only_green_mergeable_simard_authored_allowlisted_prs() {
        let author = "simard-engineer";
        let repo = "rysweet/Simard";
        let mut by_repo = HashMap::new();
        by_repo.insert(
            repo.to_string(),
            Ok(vec![
                open_pr(101, author, "MERGEABLE", "main", "SUCCESS"), // ✓ the only candidate
                open_pr(102, author, "CONFLICTING", "main", "SUCCESS"), // ✗ not MERGEABLE
                open_pr(103, author, "MERGEABLE", "main", "FAILURE"), // ✗ red CI
                open_pr(104, "human-dev", "MERGEABLE", "main", "SUCCESS"), // ✗ human-authored
                open_pr(105, author, "MERGEABLE", "release/9", "SUCCESS"), // ✗ non-allowlisted base
            ]),
        );
        let gh = ListingGh::new(by_repo);
        let (ops, seen) = survey_ops(gh.clone(), Some(author));

        let candidates = ops.survey_ready_prs(&[repo.to_string()]);

        assert_eq!(
            candidates,
            vec![pr_ref(repo, 101)],
            "only the green + MERGEABLE + Simard-authored + main-targeted PR survives"
        );
        assert_eq!(
            gh.merges(),
            0,
            "the sensor LISTS candidates — it must never merge"
        );
        assert!(
            seen.lock().unwrap().is_empty(),
            "the sensor is silent — no operator notification on a survey"
        );
    }

    #[test]
    fn survey_pushes_the_configured_author_filter_server_side() {
        // Guards the resource/robustness fix: the survey must scope the listing
        // to Simard's OWN author server-side, so a busy repo with more open PRs
        // than the fetch limit can never crowd her eligible PRs out of the
        // window (and only her PRs are transferred + parsed).
        let author = "simard-engineer";
        let repo = "rysweet/Simard";
        let mut by_repo = HashMap::new();
        by_repo.insert(
            repo.to_string(),
            Ok(vec![open_pr(401, author, "MERGEABLE", "main", "SUCCESS")]),
        );
        let gh = ListingGh::new(by_repo);
        let (ops, _seen) = survey_ops(gh.clone(), Some(author));

        let _ = ops.survey_ready_prs(&[repo.to_string()]);

        assert_eq!(
            gh.authors(),
            vec![author.to_string()],
            "the survey must forward the configured automerge author to the \
             server-side `gh pr list --author` filter"
        );
    }

    #[test]
    fn survey_matches_author_case_insensitively() {
        // Guards the case-sensitivity fix: `gh pr list --author` matches logins
        // case-insensitively server-side, so if an operator configures
        // `SIMARD_AUTOMERGE_AUTHOR` with different casing than the canonical
        // `author.login`, the in-process defense-in-depth match must NOT drop
        // the row (a silent "canary does nothing" trap). A whole-login,
        // case-insensitive equality keeps the own-PR candidate.
        let configured = "Simard-Engineer";
        let canonical = "simard-engineer";
        let repo = "rysweet/Simard";
        let mut by_repo = HashMap::new();
        by_repo.insert(
            repo.to_string(),
            Ok(vec![open_pr(
                301,
                canonical,
                "MERGEABLE",
                "main",
                "SUCCESS",
            )]),
        );
        let gh = ListingGh::new(by_repo);
        let (ops, _seen) = survey_ops(gh, Some(configured));

        assert_eq!(
            ops.survey_ready_prs(&[repo.to_string()]),
            vec![pr_ref(repo, 301)],
            "a differently-cased configured author must still match the PR's \
             canonical login (GitHub logins are unique case-insensitively)"
        );
    }

    #[test]
    fn survey_is_empty_and_queries_nothing_when_allowlist_is_empty() {
        let gh = ListingGh::new(HashMap::new());
        let (ops, _seen) = survey_ops(gh.clone(), Some("simard-engineer"));

        let candidates = ops.survey_ready_prs(&[]);

        assert!(
            candidates.is_empty(),
            "empty allowlist => empty ready_prs (autonomous merge OFF by default)"
        );
        assert!(
            gh.listed().is_empty(),
            "no repo allowlisted => the sensor must not even call `gh pr list`"
        );
    }

    #[test]
    fn survey_is_empty_when_automerge_author_is_unresolved() {
        let repo = "rysweet/Simard";
        let mut by_repo = HashMap::new();
        by_repo.insert(
            repo.to_string(),
            Ok(vec![open_pr(
                201,
                "simard-engineer",
                "MERGEABLE",
                "main",
                "SUCCESS",
            )]),
        );
        let gh = ListingGh::new(by_repo);
        // No automerge author wired => cannot distinguish own PRs => fail closed.
        let (ops, _seen) = survey_ops(gh, None);

        assert!(
            ops.survey_ready_prs(&[repo.to_string()]).is_empty(),
            "an unresolved automerge author must fail closed to an empty candidate list"
        );
    }

    #[test]
    fn survey_skips_a_repo_on_gh_error_without_panicking() {
        let author = "simard-engineer";
        let mut by_repo = HashMap::new();
        by_repo.insert("rysweet/broken".to_string(), Err(()));
        by_repo.insert(
            "rysweet/Simard".to_string(),
            Ok(vec![open_pr(301, author, "MERGEABLE", "main", "SUCCESS")]),
        );
        let gh = ListingGh::new(by_repo);
        let (ops, _seen) = survey_ops(gh, Some(author));

        let candidates =
            ops.survey_ready_prs(&["rysweet/broken".to_string(), "rysweet/Simard".to_string()]);

        assert_eq!(
            candidates,
            vec![pr_ref("rysweet/Simard", 301)],
            "a failing repo is skipped (fail-visible/closed); healthy repos still yield candidates"
        );
    }

    #[test]
    fn survey_excludes_human_authored_even_when_green_and_mergeable() {
        let author = "simard-engineer";
        let repo = "rysweet/Simard";
        let mut by_repo = HashMap::new();
        by_repo.insert(
            repo.to_string(),
            Ok(vec![
                open_pr(401, "some-human", "MERGEABLE", "main", "SUCCESS"),
                open_pr(
                    402,
                    "simard-engineer-impostor",
                    "MERGEABLE",
                    "main",
                    "SUCCESS",
                ),
            ]),
        );
        let gh = ListingGh::new(by_repo);
        let (ops, _seen) = survey_ops(gh, Some(author));

        assert!(
            ops.survey_ready_prs(&[repo.to_string()]).is_empty(),
            "author match must be EXACT — no substring/prefix — so human & look-alike PRs are excluded"
        );
    }

    // ── G3: engineer-PR gate (issue #4097) ───────────────────────────────────
    //
    // The author filter (G2) is necessary but NOT sufficient: Simard's engineer
    // PRs AND the operator's OWN review PRs are both authored by the same login
    // (`rysweet`). Turning the sensor on with only G2 would make the operator's
    // own review PRs (e.g. #3142 `cogthreads/…`) eligible to auto-merge — which
    // is unacceptable. G3 narrows candidates to PRs that PROVE Simard-origin:
    // they carry the durable `simard-autonomous` label (primary) OR ride a
    // Rust-deterministic engineer-only branch namespace (secondary). A PR that
    // has NEITHER is NEVER a candidate, even when author + CI + mergeable all
    // pass. G3 is a pure NARROWING of G2 — it can only ever remove candidates.

    /// THE safety test. An operator-shaped review PR — same author as the
    /// engineer identity (`rysweet`), green CI, MERGEABLE, targeting `main`, but
    /// with NO `simard-autonomous` label and a non-engineer branch — must be
    /// EXCLUDED. Modeled on #3142 (`cogthreads/…`), which must NEVER auto-merge.
    #[test]
    fn survey_excludes_operator_review_pr_without_label_or_engineer_branch() {
        let author = "rysweet";
        let repo = "rysweet/Simard";
        let mut by_repo = HashMap::new();
        by_repo.insert(
            repo.to_string(),
            Ok(vec![
                // #3142-shaped operator review PR: author matches, fully green,
                // but no engineer label and a `cogthreads/…` branch.
                open_pr_full(
                    3142,
                    author,
                    "MERGEABLE",
                    "main",
                    "SUCCESS",
                    "cogthreads/dashboard",
                    &[],
                ),
                // Operator manual change on a SHARED `feat/` prefix, still no label.
                open_pr_full(
                    3200,
                    author,
                    "MERGEABLE",
                    "main",
                    "SUCCESS",
                    "feat/operator-hotfix",
                    &[],
                ),
            ]),
        );
        let gh = ListingGh::new(by_repo);
        let (ops, seen) = survey_ops(gh.clone(), Some(author));

        assert!(
            ops.survey_ready_prs(&[repo.to_string()]).is_empty(),
            "an operator review PR (no engineer label, non-engineer branch) must \
             NEVER be a candidate, even when author + CI + mergeable all pass"
        );
        assert_eq!(gh.merges(), 0, "the sensor must never merge");
        assert!(
            seen.lock().unwrap().is_empty(),
            "excluding an operator PR must be silent — no notification"
        );
    }

    /// An engineer PR identified by the durable `simard-autonomous` LABEL is a
    /// candidate even on a SHARED branch prefix (`feat/…`) that an operator
    /// could also use — the label is the primary, prefix-independent marker.
    #[test]
    fn survey_includes_engineer_pr_identified_by_label() {
        let author = "rysweet";
        let repo = "rysweet/Simard";
        let mut by_repo = HashMap::new();
        by_repo.insert(
            repo.to_string(),
            Ok(vec![open_pr_full(
                500,
                author,
                "MERGEABLE",
                "main",
                "SUCCESS",
                "feat/some-shared-prefix",
                &[crate::overseer::config::SIMARD_ENGINEER_PR_LABEL],
            )]),
        );
        let gh = ListingGh::new(by_repo);
        let (ops, _seen) = survey_ops(gh, Some(author));

        assert_eq!(
            ops.survey_ready_prs(&[repo.to_string()]),
            vec![pr_ref(repo, 500)],
            "a labeled engineer PR is a candidate even on a shared branch prefix"
        );
    }

    /// An engineer PR identified ONLY by its Rust-deterministic engineer branch
    /// namespace (`engineer/…`) — with NO label — is still a candidate: the
    /// branch prefix is the secondary, defense-in-depth marker for the case
    /// where the best-effort label was not applied.
    #[test]
    fn survey_includes_engineer_pr_identified_by_branch_prefix() {
        let author = "rysweet";
        let repo = "rysweet/Simard";
        let mut by_repo = HashMap::new();
        by_repo.insert(
            repo.to_string(),
            Ok(vec![
                open_pr_full(
                    600,
                    author,
                    "MERGEABLE",
                    "main",
                    "SUCCESS",
                    "engineer/600-ab12cd34",
                    &[],
                ),
                open_pr_full(
                    601,
                    author,
                    "MERGEABLE",
                    "main",
                    "SUCCESS",
                    "chore/advisory-rustsec-2024-0001",
                    &[],
                ),
            ]),
        );
        let gh = ListingGh::new(by_repo);
        let (ops, _seen) = survey_ops(gh, Some(author));

        assert_eq!(
            ops.survey_ready_prs(&[repo.to_string()]),
            vec![pr_ref(repo, 600), pr_ref(repo, 601)],
            "engineer-only branch namespaces qualify even without the label"
        );
    }

    /// Mixed repo, shared author: only the engineer-identified PRs survive G3;
    /// the operator review PR is excluded. This is the exact enablement scenario
    /// that was previously impossible (both authored by `rysweet`).
    #[test]
    fn survey_separates_engineer_prs_from_operator_prs_under_shared_author() {
        let author = "rysweet";
        let repo = "rysweet/Simard";
        let mut by_repo = HashMap::new();
        by_repo.insert(
            repo.to_string(),
            Ok(vec![
                // ✓ engineer PR by label
                open_pr_full(
                    700,
                    author,
                    "MERGEABLE",
                    "main",
                    "SUCCESS",
                    "feat/engineer-work",
                    &[crate::overseer::config::SIMARD_ENGINEER_PR_LABEL],
                ),
                // ✓ engineer PR by branch namespace
                open_pr_full(
                    701,
                    author,
                    "MERGEABLE",
                    "main",
                    "SUCCESS",
                    "engineer/701-ffff0000",
                    &[],
                ),
                // ✗ operator review PR — same author, green, but neither marker
                open_pr_full(
                    3142,
                    author,
                    "MERGEABLE",
                    "main",
                    "SUCCESS",
                    "cogthreads/review",
                    &[],
                ),
            ]),
        );
        let gh = ListingGh::new(by_repo);
        let (ops, _seen) = survey_ops(gh, Some(author));

        assert_eq!(
            ops.survey_ready_prs(&[repo.to_string()]),
            vec![pr_ref(repo, 700), pr_ref(repo, 701)],
            "under a shared author, only engineer-marked PRs are candidates; the \
             operator review PR is excluded"
        );
    }

    #[test]
    fn survey_default_trait_seam_is_empty() {
        struct MinimalPrOps;
        impl PrOps for MinimalPrOps {
            fn verify(&self, _r: &str, _p: u32) -> Result<VerifyReport, OverseerError> {
                unreachable!()
            }
            fn merge(&self, _r: &str, _p: u32) -> Result<(), OverseerError> {
                unreachable!()
            }
            fn resolve_conflict(&self, _r: &str, _p: u32) -> Result<(), OverseerError> {
                unreachable!()
            }
        }
        assert!(
            MinimalPrOps
                .survey_ready_prs(&["rysweet/Simard".to_string()])
                .is_empty(),
            "the PrOps survey seam defaults to empty (default-off, fail-closed)"
        );
    }
}
