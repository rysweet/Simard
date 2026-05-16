//! Merge authority — Simard's gated authority to squash-merge a pull request
//! once it has independently demonstrated merge-readiness.
//!
//! See `prompt_assets/simard/engineer_system.md` (Merge-Ready Contract) and
//! `~/.copilot/skills/merge-ready/SKILL.md` for the canonical six criteria.
//!
//! Pipeline:
//! 1. `gh pr view <PR> --json body,statusCheckRollup,mergeable,reviewDecision,baseRefName`
//! 2. **Objective gates** (deterministic, never agentic):
//!    - `baseRefName` is in the configured allow-list (default `["main"]`,
//!      overridable via the `SIMARD_MERGE_BASE_ALLOWLIST` env var as a
//!      comma-separated list). This is the **first** gate evaluated so a PR
//!      targeting a stale or wrong base branch (the PR #1549 footgun) is
//!      refused before any other inspection runs.
//!    - `mergeable == "MERGEABLE"`.
//!    - Every entry in `statusCheckRollup` is `SUCCESS`, `NEUTRAL`, or
//!      `SKIPPED`. Any `FAILURE`, `CANCELLED`, `TIMED_OUT`, `STARTUP_FAILURE`,
//!      `ACTION_REQUIRED`, `PENDING`, `QUEUED`, or `IN_PROGRESS` blocks the merge.
//! 3. **Agentic gate** ([`super::merge_judge::MergeJudge`]): a prompt-driven
//!    judge reads the PR body and returns a structured verdict on whether the
//!    merge-ready skill criteria are satisfied. The judge's prompt at
//!    `prompt_assets/simard/merge_readiness_judge.md` is the single source of
//!    truth for the evidence criteria — editing the skill template is enough
//!    to evolve what the judge accepts. **No hardcoded heading lists, byte
//!    thresholds, or bracket heuristics live in this module any more.**
//! 4. If all gates pass: `gh pr merge <PR> --squash --delete-branch
//!    --repo rysweet/Simard` and return [`MergeOutcome::Merged`].
//! 5. Otherwise return [`MergeOutcome::Refused`] with the first failing
//!    objective gate, or the judge's blocker summary if every objective gate
//!    passed.
//!
//! TODO(brain-wiring): the OODA brain currently has no action kind for "merge
//! a PR I worked on" (issue #1868). When the brain grows a `merge_pr` action,
//! wire [`merge_pr_if_merge_ready`] in via `src/ooda_actions/`. Until then it is
//! reachable via the operator CLI subcommand `simard merge-pr <PR>` (see
//! `src/operator_cli/merge.rs`) and via direct library calls.

use crate::error::{SimardError, SimardResult};

use super::merge_judge::{JudgeOutcome, MergeJudge, Verdict, build_merge_judge};

/// Result of a merge-authority evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The PR satisfied every gate and was successfully squash-merged.
    Merged { pr_number: u32, repo: String },
    /// The PR did not satisfy a gate, or `gh pr merge` itself refused.
    /// `reason` is a single human-readable sentence the operator can act on.
    Refused { pr_number: u32, reason: String },
}

/// Snapshot of `gh pr view --json body,statusCheckRollup,mergeable,reviewDecision,baseRefName`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PrSnapshot {
    pub body: String,
    pub mergeable: String,
    pub review_decision: String,
    pub checks: Vec<CheckRollupEntry>,
    /// `baseRefName` from `gh pr view` — the branch this PR will merge **into**.
    /// Compared against [`base_allowlist_from_env`] by the first gate so PRs
    /// targeting stale or wrong base branches are refused early.
    pub base_ref_name: String,
}

/// One row from `statusCheckRollup`. Both check runs and statuses get
/// normalised into this shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckRollupEntry {
    /// Display name (`name` for check runs, `context` for statuses).
    pub name: String,
    /// One of: `SUCCESS`, `NEUTRAL`, `SKIPPED`, `FAILURE`, `CANCELLED`,
    /// `TIMED_OUT`, `STARTUP_FAILURE`, `ACTION_REQUIRED`, `PENDING`,
    /// `QUEUED`, `IN_PROGRESS`, or any state the gh CLI invents next week.
    /// We treat unknown values as failing-by-default.
    pub state: String,
}

/// Abstract `gh pr` operations used by [`merge_pr_if_merge_ready`]. The trait
/// keeps the evaluation logic testable; production wires it to
/// [`RealPrGhClient`] which shells out to `gh`.
pub trait PrGhClient {
    /// `gh pr view <pr> --repo <repo> --json body,statusCheckRollup,mergeable,reviewDecision,baseRefName`.
    fn view_pr(&self, repo: &str, pr_number: u32) -> SimardResult<PrSnapshot>;
    /// `gh pr merge <pr> --squash --delete-branch --repo <repo>`.
    fn squash_merge(&self, repo: &str, pr_number: u32) -> SimardResult<()>;
}

/// Production implementation that shells out to the `gh` binary.
#[derive(Default)]
pub struct RealPrGhClient;

impl RealPrGhClient {
    pub fn new() -> Self {
        Self
    }
}

impl PrGhClient for RealPrGhClient {
    fn view_pr(&self, repo: &str, pr_number: u32) -> SimardResult<PrSnapshot> {
        let pr = pr_number.to_string();
        let output = std::process::Command::new("gh")
            .args([
                "pr",
                "view",
                &pr,
                "--repo",
                repo,
                "--json",
                "body,statusCheckRollup,mergeable,reviewDecision,baseRefName",
            ])
            .output()
            .map_err(|e| SimardError::MergeAuthorityGhCommandFailed {
                reason: format!("failed to spawn `gh pr view`: {e}"),
            })?;
        if !output.status.success() {
            return Err(SimardError::MergeAuthorityGhCommandFailed {
                reason: format!(
                    "`gh pr view {pr} --repo {repo}` exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        parse_pr_view_json(&output.stdout)
    }

    fn squash_merge(&self, repo: &str, pr_number: u32) -> SimardResult<()> {
        let pr = pr_number.to_string();
        let output = std::process::Command::new("gh")
            .args([
                "pr",
                "merge",
                &pr,
                "--repo",
                repo,
                "--squash",
                "--delete-branch",
            ])
            .output()
            .map_err(|e| SimardError::MergeAuthorityGhCommandFailed {
                reason: format!("failed to spawn `gh pr merge`: {e}"),
            })?;
        if !output.status.success() {
            return Err(SimardError::MergeAuthorityGhCommandFailed {
                reason: format!(
                    "`gh pr merge {pr} --repo {repo} --squash --delete-branch` exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        Ok(())
    }
}

/// Parse `gh pr view --json body,statusCheckRollup,mergeable,reviewDecision,baseRefName`
/// stdout into a [`PrSnapshot`]. Public so the CLI can reuse it for dry-run
/// flows; tests cover both happy and malformed paths.
pub fn parse_pr_view_json(stdout: &[u8]) -> SimardResult<PrSnapshot> {
    #[derive(serde::Deserialize)]
    struct Raw {
        #[serde(default)]
        body: String,
        #[serde(default)]
        mergeable: String,
        #[serde(default, rename = "reviewDecision")]
        review_decision: String,
        #[serde(default, rename = "statusCheckRollup")]
        status_check_rollup: Vec<RawCheck>,
        #[serde(default, rename = "baseRefName")]
        base_ref_name: String,
    }
    #[derive(serde::Deserialize)]
    struct RawCheck {
        // Check runs use `name`+`conclusion`/`status`; statuses use `context`+`state`.
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        context: Option<String>,
        #[serde(default)]
        conclusion: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        state: Option<String>,
    }
    let raw: Raw = serde_json::from_slice(stdout).map_err(|e| {
        SimardError::MergeAuthorityEvaluationFailed {
            reason: format!("could not parse `gh pr view` JSON: {e}"),
        }
    })?;
    let checks = raw
        .status_check_rollup
        .into_iter()
        .map(|c| {
            let name = c
                .name
                .or(c.context)
                .unwrap_or_else(|| "<unnamed-check>".to_string());
            // gh reports a check-run as IN_PROGRESS via `status` until
            // `conclusion` is populated; once complete `conclusion` is the
            // truthful field. Statuses use `state`. Fall through in that
            // order so a half-finished check doesn't masquerade as success.
            let state = match (c.conclusion, c.status, c.state) {
                (Some(s), _, _) if !s.is_empty() => s,
                (_, Some(s), _) if !s.is_empty() => s,
                (_, _, Some(s)) if !s.is_empty() => s,
                _ => "UNKNOWN".to_string(),
            };
            CheckRollupEntry { name, state }
        })
        .collect();
    Ok(PrSnapshot {
        body: raw.body,
        mergeable: raw.mergeable,
        review_decision: raw.review_decision,
        checks,
        base_ref_name: raw.base_ref_name,
    })
}

/// Env var that overrides the base-branch allow-list (comma-separated).
/// Empty entries are ignored. Falls back to `["main"]` if unset/empty.
pub const BASE_ALLOWLIST_ENV: &str = "SIMARD_MERGE_BASE_ALLOWLIST";

/// The default base-branch allow-list when the env var is unset.
pub const DEFAULT_BASE_ALLOWLIST: &[&str] = &["main"];

/// Read [`BASE_ALLOWLIST_ENV`] from the environment, splitting on commas.
/// Returns the default list (`["main"]`) if the env var is unset, empty, or
/// contains only whitespace/empty entries.
pub fn base_allowlist_from_env() -> Vec<String> {
    let raw = std::env::var(BASE_ALLOWLIST_ENV).unwrap_or_default();
    let parsed: Vec<String> = raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if parsed.is_empty() {
        DEFAULT_BASE_ALLOWLIST
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        parsed
    }
}

/// First objective gate that fails (in order). Returns `Ok(())` if every
/// objective gate passes. `base_allowlist` is the set of base branches a PR
/// is allowed to target; production callers obtain this from
/// [`base_allowlist_from_env`].
///
/// **Objective gates only** — evidence judgment is handled separately by the
/// agentic [`MergeJudge`] (see [`merge_pr_if_merge_ready_with_judge`]). Every
/// gate here is a fact that can be checked without reading the PR body.
fn evaluate_objective_gates(
    snapshot: &PrSnapshot,
    base_allowlist: &[String],
) -> Result<(), String> {
    // Gate 0 (highest priority): base-branch allow-list.
    //
    // A PR whose `baseRefName` is not in the allow-list is the PR #1549
    // footgun: branched from a stale parent so the diff includes thousands
    // of unrelated lines that look like deletions when targeted at main.
    // Refuse early — before any other inspection runs — and tell the
    // operator exactly how to re-target.
    if !base_allowlist
        .iter()
        .any(|allowed| allowed == &snapshot.base_ref_name)
    {
        return Err(format!(
            "PR base branch '{}' is not in the merge allow-list ({}). \
             Re-target this PR to an allowed base and rebase before retrying: \
             `gh pr edit <PR> --base {}` followed by `git rebase origin/{}`.",
            snapshot.base_ref_name,
            base_allowlist.join(", "),
            base_allowlist.first().map(String::as_str).unwrap_or("main"),
            base_allowlist.first().map(String::as_str).unwrap_or("main"),
        ));
    }

    // Gate 1: mergeable
    if snapshot.mergeable != "MERGEABLE" {
        return Err(format!(
            "PR mergeable status is '{}' (expected 'MERGEABLE')",
            snapshot.mergeable
        ));
    }
    // Gate 2: every check is success-ish
    for check in &snapshot.checks {
        if !is_passing_state(&check.state) {
            return Err(format!(
                "CI check '{}' has state '{}' (expected SUCCESS/NEUTRAL/SKIPPED)",
                check.name, check.state
            ));
        }
    }
    Ok(())
}

fn is_passing_state(state: &str) -> bool {
    matches!(state, "SUCCESS" | "NEUTRAL" | "SKIPPED")
}

/// Evaluate the merge-ready gates for `pr_number` against `repo`. If every
/// gate passes, squash-merge with branch deletion and return
/// [`MergeOutcome::Merged`]. Otherwise return [`MergeOutcome::Refused`] with
/// the single most-actionable reason (the first failing objective gate, or
/// the judge's blocker summary if every objective gate passed).
///
/// The base-branch allow-list is read from the `SIMARD_MERGE_BASE_ALLOWLIST`
/// environment variable (comma-separated, default `"main"`). See
/// [`base_allowlist_from_env`].
///
/// The agentic [`MergeJudge`] is constructed via [`build_merge_judge`], which
/// resolves an LLM provider via the same path the OODA brains use. If no
/// provider is configured, the judge refuses with an actionable "judge
/// unavailable" message rather than silently falling back to brittle
/// string-matching heuristics.
///
/// Errors (as opposed to [`MergeOutcome::Refused`]) only surface when we
/// could not even *evaluate* the PR — `gh` failed to run, returned malformed
/// JSON, the judge submitter errored at the network layer, or `gh pr merge`
/// itself failed despite the gates being satisfied.
pub fn merge_pr_if_merge_ready(
    pr_number: u32,
    repo: &str,
    gh: &dyn PrGhClient,
) -> SimardResult<MergeOutcome> {
    merge_pr_if_merge_ready_with_allowlist(pr_number, repo, gh, &base_allowlist_from_env())
}

/// Variant of [`merge_pr_if_merge_ready`] that takes an explicit base-branch
/// allow-list. Used by tests; production paths should call the env-driven
/// [`merge_pr_if_merge_ready`] instead.
pub fn merge_pr_if_merge_ready_with_allowlist(
    pr_number: u32,
    repo: &str,
    gh: &dyn PrGhClient,
    base_allowlist: &[String],
) -> SimardResult<MergeOutcome> {
    let judge = build_merge_judge();
    merge_pr_if_merge_ready_with_judge(pr_number, repo, gh, base_allowlist, judge.as_ref())
}

/// Full-control entrypoint that takes an explicit [`MergeJudge`]. Used by
/// tests (with a stub judge) and by future call sites that want to provide
/// their own judge implementation.
///
/// Pipeline:
/// 1. Fetch PR snapshot via `gh`.
/// 2. Evaluate objective gates (base-branch, mergeable, CI). If any fails,
///    return `Refused` immediately — do not even call the judge.
/// 3. Call the judge. If the verdict is anything other than `Ready`, return
///    `Refused` with the judge's structured blocker summary.
/// 4. Squash-merge.
pub fn merge_pr_if_merge_ready_with_judge(
    pr_number: u32,
    repo: &str,
    gh: &dyn PrGhClient,
    base_allowlist: &[String],
    judge: &dyn MergeJudge,
) -> SimardResult<MergeOutcome> {
    let snapshot = gh.view_pr(repo, pr_number)?;
    if let Err(reason) = evaluate_objective_gates(&snapshot, base_allowlist) {
        return Ok(MergeOutcome::Refused { pr_number, reason });
    }
    let outcome: JudgeOutcome = judge.judge(pr_number, repo, &snapshot)?;
    match outcome.verdict {
        Verdict::Ready => {
            gh.squash_merge(repo, pr_number)?;
            Ok(MergeOutcome::Merged {
                pr_number,
                repo: repo.to_string(),
            })
        }
        Verdict::NotReady | Verdict::Unclear => Ok(MergeOutcome::Refused {
            pr_number,
            reason: outcome.summary(),
        }),
    }
}

// ─────────────────────────── Tests ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stewardship::merge_judge::{Blocker, JudgeOutcome, MergeJudge, Verdict};
    use std::sync::Mutex;

    // ─── Fixtures ──────────────────────────────────────────────────────────

    /// A non-trivial PR body. After the agentic-judge refactor the body is
    /// just an opaque blob the judge inspects; the merge_authority module no
    /// longer parses it. We keep a realistic example here so test failures
    /// involving the body remain easy to read.
    fn good_pr_body() -> String {
        "# feat: example PR\n\
         \n\
         ## Merge readiness\n\
         \n\
         ### QA-team evidence\n\
         Scenarios under tests/scenarios/, 12/12 green.\n\
         \n\
         ### Documentation\n\
         Updated docs/concepts/merge-authority.md.\n\
         \n\
         ### Quality-audit\n\
         Three SEEK→VALIDATE→FIX cycles, last clean.\n\
         \n\
         ### CI\n\
         All required checks green.\n\
         \n\
         ### Scope\n\
         Only intended files touched.\n\
         \n\
         ### Verdict\n\
         Ready to merge.\n"
            .to_string()
    }

    fn good_snapshot() -> PrSnapshot {
        PrSnapshot {
            body: good_pr_body(),
            mergeable: "MERGEABLE".to_string(),
            review_decision: "APPROVED".to_string(),
            checks: vec![
                CheckRollupEntry {
                    name: "build".into(),
                    state: "SUCCESS".into(),
                },
                CheckRollupEntry {
                    name: "clippy".into(),
                    state: "SUCCESS".into(),
                },
                CheckRollupEntry {
                    name: "license-scan".into(),
                    state: "NEUTRAL".into(),
                },
            ],
            base_ref_name: "main".to_string(),
        }
    }

    fn default_allowlist() -> Vec<String> {
        vec!["main".to_string()]
    }

    // ─── PR-gh client mock (unchanged from pre-refactor) ──────────────────

    #[derive(Default)]
    struct FakePrGhClient {
        snapshot: Mutex<Option<SimardResult<PrSnapshot>>>,
        merge_result: Mutex<Option<SimardResult<()>>>,
        view_calls: Mutex<Vec<(String, u32)>>,
        merge_calls: Mutex<Vec<(String, u32)>>,
    }

    impl FakePrGhClient {
        fn new() -> Self {
            Self::default()
        }
        fn seed_view(&self, result: SimardResult<PrSnapshot>) {
            *self.snapshot.lock().unwrap() = Some(result);
        }
        fn seed_merge(&self, result: SimardResult<()>) {
            *self.merge_result.lock().unwrap() = Some(result);
        }
        fn merge_call_count(&self) -> usize {
            self.merge_calls.lock().unwrap().len()
        }
    }

    impl PrGhClient for FakePrGhClient {
        fn view_pr(&self, repo: &str, pr: u32) -> SimardResult<PrSnapshot> {
            self.view_calls.lock().unwrap().push((repo.to_string(), pr));
            self.snapshot
                .lock()
                .unwrap()
                .clone()
                .expect("FakePrGhClient: no view_pr response seeded")
        }
        fn squash_merge(&self, repo: &str, pr: u32) -> SimardResult<()> {
            self.merge_calls
                .lock()
                .unwrap()
                .push((repo.to_string(), pr));
            self.merge_result.lock().unwrap().clone().unwrap_or(Ok(()))
        }
    }

    // ─── Merge-judge mock (new; replaces hardcoded evidence gates) ────────

    struct FakeMergeJudge {
        canned: Mutex<Option<SimardResult<JudgeOutcome>>>,
        calls: Mutex<u32>,
    }

    impl FakeMergeJudge {
        fn ready() -> Self {
            Self::new(Ok(JudgeOutcome {
                verdict: Verdict::Ready,
                rationale: "all six skill criteria substantive (test fixture)".to_string(),
                blockers: vec![],
            }))
        }
        fn not_ready_with(blockers: Vec<Blocker>) -> Self {
            Self::new(Ok(JudgeOutcome {
                verdict: Verdict::NotReady,
                rationale: "test: judge said not_ready".to_string(),
                blockers,
            }))
        }
        fn unclear() -> Self {
            Self::new(Ok(JudgeOutcome {
                verdict: Verdict::Unclear,
                rationale: "test: judge said unclear".to_string(),
                blockers: vec![],
            }))
        }
        fn errored() -> Self {
            Self::new(Err(SimardError::AdapterInvocationFailed {
                base_type: "merge-readiness-judge".into(),
                reason: "test: simulated network failure".into(),
            }))
        }
        fn new(canned: SimardResult<JudgeOutcome>) -> Self {
            Self {
                canned: Mutex::new(Some(canned)),
                calls: Mutex::new(0),
            }
        }
        fn call_count(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    impl MergeJudge for FakeMergeJudge {
        fn judge(
            &self,
            _pr: u32,
            _repo: &str,
            _snapshot: &PrSnapshot,
        ) -> SimardResult<JudgeOutcome> {
            *self.calls.lock().unwrap() += 1;
            self.canned
                .lock()
                .unwrap()
                .clone()
                .expect("FakeMergeJudge: no canned response")
        }
    }

    // Convenience: every test below calls the with_judge entrypoint directly
    // so the judge dependency is explicit and there is no hidden global state.
    fn run(
        pr: u32,
        repo: &str,
        gh: &dyn PrGhClient,
        allow: &[String],
        judge: &dyn MergeJudge,
    ) -> SimardResult<MergeOutcome> {
        merge_pr_if_merge_ready_with_judge(pr, repo, gh, allow, judge)
    }

    // ─── Happy path: objective gates pass + judge says ready ──────────────

    #[test]
    fn merges_when_objective_gates_pass_and_judge_says_ready() {
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(good_snapshot()));
        gh.seed_merge(Ok(()));
        let judge = FakeMergeJudge::ready();

        let outcome = run(1500, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap();

        assert_eq!(
            outcome,
            MergeOutcome::Merged {
                pr_number: 1500,
                repo: "rysweet/Simard".to_string(),
            }
        );
        assert_eq!(gh.merge_call_count(), 1);
        assert_eq!(judge.call_count(), 1, "judge must be called exactly once");
    }

    // ─── Judge verdicts ───────────────────────────────────────────────────

    #[test]
    fn refuses_when_judge_says_not_ready_and_surfaces_blockers() {
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(good_snapshot()));
        let judge = FakeMergeJudge::not_ready_with(vec![
            Blocker {
                section: "Quality-audit".into(),
                severity: "high".into(),
                observation: "single sentence, no SHAs".into(),
                fix: "run three SEEK→VALIDATE→FIX cycles".into(),
            },
            Blocker {
                section: "CI".into(),
                severity: "medium".into(),
                observation: "no run link".into(),
                fix: "add gh pr checks output".into(),
            },
        ]);

        let outcome = run(42, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap();

        match outcome {
            MergeOutcome::Refused { pr_number, reason } => {
                assert_eq!(pr_number, 42);
                assert!(reason.contains("not_ready"), "{reason}");
                assert!(reason.contains("Quality-audit"), "{reason}");
                assert!(reason.contains("CI"), "{reason}");
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        assert_eq!(gh.merge_call_count(), 0, "must not merge on not_ready");
    }

    #[test]
    fn refuses_when_judge_says_unclear() {
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(good_snapshot()));
        let judge = FakeMergeJudge::unclear();

        let outcome = run(7, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap();

        match outcome {
            MergeOutcome::Refused { reason, .. } => {
                assert!(reason.contains("unclear"), "{reason}");
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        assert_eq!(gh.merge_call_count(), 0);
    }

    #[test]
    fn judge_errors_propagate_as_simard_error() {
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(good_snapshot()));
        let judge = FakeMergeJudge::errored();

        let err = run(7, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap_err();
        match err {
            SimardError::AdapterInvocationFailed { base_type, reason } => {
                assert_eq!(base_type, "merge-readiness-judge");
                assert!(reason.contains("simulated network failure"), "{reason}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert_eq!(gh.merge_call_count(), 0);
    }

    // ─── Objective gates: CI, mergeable, base branch ──────────────────────

    #[test]
    fn refuses_on_ci_failure() {
        let mut snap = good_snapshot();
        snap.checks.push(CheckRollupEntry {
            name: "integration-tests".into(),
            state: "FAILURE".into(),
        });
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let judge = FakeMergeJudge::ready();

        let outcome = run(7, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap();
        match outcome {
            MergeOutcome::Refused { reason, .. } => {
                assert!(reason.contains("integration-tests"), "{reason}");
                assert!(reason.contains("FAILURE"), "{reason}");
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        assert_eq!(gh.merge_call_count(), 0);
        assert_eq!(
            judge.call_count(),
            0,
            "objective gate failure must not invoke the judge"
        );
    }

    #[test]
    fn refuses_on_pending_check() {
        let mut snap = good_snapshot();
        snap.checks.push(CheckRollupEntry {
            name: "slow-bench".into(),
            state: "PENDING".into(),
        });
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let judge = FakeMergeJudge::ready();

        let outcome = run(7, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap();
        assert!(matches!(outcome, MergeOutcome::Refused { .. }));
        assert_eq!(judge.call_count(), 0);
    }

    #[test]
    fn refuses_when_mergeable_conflicting() {
        let mut snap = good_snapshot();
        snap.mergeable = "CONFLICTING".to_string();
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let judge = FakeMergeJudge::ready();

        let outcome = run(7, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap();
        match outcome {
            MergeOutcome::Refused { reason, .. } => {
                assert!(reason.contains("CONFLICTING"), "{reason}");
                assert!(reason.contains("MERGEABLE"), "{reason}");
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        assert_eq!(gh.merge_call_count(), 0);
        assert_eq!(judge.call_count(), 0);
    }

    #[test]
    fn refuses_when_mergeable_unknown() {
        let mut snap = good_snapshot();
        snap.mergeable = "UNKNOWN".to_string();
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let judge = FakeMergeJudge::ready();

        let outcome = run(7, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap();
        assert!(matches!(outcome, MergeOutcome::Refused { .. }));
    }

    // ─── gh failures bubble through ───────────────────────────────────────

    #[test]
    fn propagates_gh_view_failure() {
        let gh = FakePrGhClient::new();
        gh.seed_view(Err(SimardError::MergeAuthorityGhCommandFailed {
            reason: "gh: not found".into(),
        }));
        let judge = FakeMergeJudge::ready();
        let err = run(7, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap_err();
        assert!(matches!(
            err,
            SimardError::MergeAuthorityGhCommandFailed { .. }
        ));
    }

    #[test]
    fn propagates_gh_merge_failure_after_passing_gates() {
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(good_snapshot()));
        gh.seed_merge(Err(SimardError::MergeAuthorityGhCommandFailed {
            reason: "branch protection requires CODEOWNERS approval".into(),
        }));
        let judge = FakeMergeJudge::ready();
        let err = run(1500, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap_err();
        match err {
            SimardError::MergeAuthorityGhCommandFailed { reason } => {
                assert!(reason.contains("branch protection"), "{reason}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // ─── parse_pr_view_json ───────────────────────────────────────────────

    #[test]
    fn parses_check_run_with_conclusion() {
        let json = br#"{
            "body": "hi",
            "mergeable": "MERGEABLE",
            "reviewDecision": "APPROVED",
            "statusCheckRollup": [
                {"name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"name": "lint",  "status": "IN_PROGRESS", "conclusion": ""}
            ]
        }"#;
        let snap = parse_pr_view_json(json).unwrap();
        assert_eq!(snap.checks.len(), 2);
        assert_eq!(snap.checks[0].state, "SUCCESS");
        assert_eq!(snap.checks[1].state, "IN_PROGRESS");
    }

    #[test]
    fn parses_status_with_state_and_context() {
        let json = br#"{
            "body": "hi",
            "mergeable": "MERGEABLE",
            "reviewDecision": "REVIEW_REQUIRED",
            "statusCheckRollup": [
                {"context": "ci/legacy", "state": "SUCCESS"},
                {"context": "ci/old",    "state": "PENDING"}
            ]
        }"#;
        let snap = parse_pr_view_json(json).unwrap();
        assert_eq!(snap.checks.len(), 2);
        assert_eq!(snap.checks[0].name, "ci/legacy");
        assert_eq!(snap.checks[0].state, "SUCCESS");
        assert_eq!(snap.checks[1].state, "PENDING");
    }

    #[test]
    fn parse_pr_view_json_rejects_garbage() {
        let err = parse_pr_view_json(b"not json at all").unwrap_err();
        assert!(matches!(
            err,
            SimardError::MergeAuthorityEvaluationFailed { .. }
        ));
    }

    // ─── Base-branch allow-list gate (PR #1549 footgun) ──────────────────

    #[test]
    fn refuses_when_base_ref_not_in_allowlist() {
        let mut snap = good_snapshot();
        snap.base_ref_name = "feat/some-stale-parent".to_string();
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let judge = FakeMergeJudge::ready();

        let outcome = run(1549, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap();
        match outcome {
            MergeOutcome::Refused { pr_number, reason } => {
                assert_eq!(pr_number, 1549);
                assert!(
                    reason.contains("feat/some-stale-parent"),
                    "reason should report the detected base: {reason}"
                );
                assert!(
                    reason.contains("main"),
                    "reason should list the allowed base(s): {reason}"
                );
                assert!(
                    reason.contains("gh pr edit"),
                    "reason should hint at the re-target command: {reason}"
                );
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        assert_eq!(gh.merge_call_count(), 0);
        assert_eq!(
            judge.call_count(),
            0,
            "base-branch refusal must short-circuit before the judge"
        );
    }

    #[test]
    fn allows_pr_when_base_in_custom_allowlist() {
        let mut snap = good_snapshot();
        snap.base_ref_name = "release/0.18".to_string();
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        gh.seed_merge(Ok(()));
        let allowlist = vec!["main".to_string(), "release/0.18".to_string()];
        let judge = FakeMergeJudge::ready();
        let outcome = run(2000, "rysweet/Simard", &gh, &allowlist, &judge).unwrap();
        assert_eq!(
            outcome,
            MergeOutcome::Merged {
                pr_number: 2000,
                repo: "rysweet/Simard".to_string(),
            }
        );
        assert_eq!(gh.merge_call_count(), 1);
        assert_eq!(judge.call_count(), 1);
    }

    /// The objective base-branch gate must short-circuit before the judge is
    /// consulted, regardless of what the judge would have said. This pins
    /// the order so a future refactor can't reverse it.
    #[test]
    fn base_branch_gate_runs_before_judge() {
        let mut snap = good_snapshot();
        snap.base_ref_name = "wrong-base".to_string();
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        // Judge would say ready, but the objective gate must win.
        let judge = FakeMergeJudge::ready();

        let outcome = run(7, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap();
        match outcome {
            MergeOutcome::Refused { reason, .. } => {
                assert!(reason.contains("wrong-base"), "{reason}");
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        assert_eq!(judge.call_count(), 0);
    }

    // ─── base_allowlist_from_env ──────────────────────────────────────────
    //
    // Env mutation isn't thread-safe; cargo runs tests in parallel by
    // default. Serialize every test that touches BASE_ALLOWLIST_ENV through
    // this mutex so no two of them race.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn base_allowlist_from_env_default_is_main() {
        let _g = env_lock().lock().unwrap();
        // SAFETY: serialized via env_lock above.
        unsafe {
            std::env::remove_var(BASE_ALLOWLIST_ENV);
        }
        let list = base_allowlist_from_env();
        assert_eq!(list, vec!["main".to_string()]);
    }

    #[test]
    fn base_allowlist_from_env_splits_and_trims() {
        let _g = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var(BASE_ALLOWLIST_ENV, "main, release/0.18 ,, dev");
        }
        let list = base_allowlist_from_env();
        unsafe {
            std::env::remove_var(BASE_ALLOWLIST_ENV);
        }
        assert_eq!(
            list,
            vec![
                "main".to_string(),
                "release/0.18".to_string(),
                "dev".to_string(),
            ]
        );
    }

    #[test]
    fn base_allowlist_from_env_empty_string_falls_back_to_default() {
        let _g = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var(BASE_ALLOWLIST_ENV, "   ,  , ");
        }
        let list = base_allowlist_from_env();
        unsafe {
            std::env::remove_var(BASE_ALLOWLIST_ENV);
        }
        assert_eq!(list, vec!["main".to_string()]);
    }

    #[test]
    fn parse_pr_view_json_includes_base_ref_name() {
        let json = br#"{
            "body": "hi",
            "mergeable": "MERGEABLE",
            "reviewDecision": "APPROVED",
            "baseRefName": "main",
            "statusCheckRollup": []
        }"#;
        let snap = parse_pr_view_json(json).unwrap();
        assert_eq!(snap.base_ref_name, "main");
    }

    #[test]
    fn parse_pr_view_json_missing_base_ref_name_defaults_empty() {
        // Older `gh` versions or unusual payloads may omit baseRefName.
        // We default to the empty string, which then fails the base
        // allow-list gate — strictly safer than guessing "main".
        let json = br#"{
            "body": "hi",
            "mergeable": "MERGEABLE",
            "reviewDecision": "APPROVED",
            "statusCheckRollup": []
        }"#;
        let snap = parse_pr_view_json(json).unwrap();
        assert_eq!(snap.base_ref_name, "");

        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(snap));
        let judge = FakeMergeJudge::ready();
        let outcome = run(99, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap();
        assert!(
            matches!(outcome, MergeOutcome::Refused { .. }),
            "missing baseRefName must fail the gate, not silently pass"
        );
    }
}
