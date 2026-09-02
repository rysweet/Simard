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
//! 4. If all gates pass: `gh pr merge <PR> --repo <repo> --squash
//!    --delete-branch` (the target repo is a parameter — defaulting to
//!    `rysweet/Simard` at the CLI — so the same gated path lands cross-repo
//!    PRs) and return [`MergeOutcome::Merged`].
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

/// Snapshot of `gh pr view --json body,statusCheckRollup,mergeable,reviewDecision,baseRefName,labels`.
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
    /// `labels` (names) from `gh pr view`. Drives the creative-idea
    /// human-review gate: a PR carrying
    /// [`crate::creative_ideas::CREATIVE_IDEA_PR_LABEL`] is never auto-merged.
    pub labels: Vec<String>,
    /// `isDraft` from `gh pr view --json ...,isDraft`. A draft PR can NEVER be
    /// merged server-side (`gh pr merge` returns "Pull Request is still a
    /// draft"), so the deterministic rail refuses it. Fail-closed: `None`
    /// (field absent/unknown) is treated as NOT-mergeable — the draft gate
    /// admits ONLY `Some(false)`. Mirrors [`OpenPrSummary::is_draft`].
    pub is_draft: Option<bool>,
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

/// One open-PR summary used by the dashboard's Merge Readiness panel
/// (#1880). Sourced from
/// `gh pr list --json number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url,author,labels`.
/// Mirrors [`PrSnapshot`] without `body` or `review_decision` — the panel
/// only renders the cheap deterministic gates per PR.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct OpenPrSummary {
    pub number: u32,
    pub title: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub mergeable: String,
    pub checks: Vec<CheckRollupEntry>,
    pub url: String,
    /// `author.login` from `gh pr list --json ...,author`. Used by the
    /// autonomous-self-merge sensor (#4097) to tell Simard's OWN PRs from a
    /// human's — an empty author (missing object) can never equal a configured
    /// automerge author, so it fails closed. Not read by the dashboard panel.
    pub author: String,
    /// Label names from `gh pr list --json ...,labels`. The autonomous-self-merge
    /// sensor (#4097) reads this to positively identify Simard's OWN engineer PRs
    /// (they carry [`SIMARD_ENGINEER_PR_LABEL`]) and separate them from the
    /// operator's own review PRs when both share the same author login. Nameless
    /// labels are dropped at parse time. Not read by the dashboard panel.
    ///
    /// [`SIMARD_ENGINEER_PR_LABEL`]: crate::overseer::config::SIMARD_ENGINEER_PR_LABEL
    pub labels: Vec<String>,
    /// `isDraft` from `gh pr list --json ...,isDraft`. A draft PR can NEVER be
    /// merged server-side (`gh pr merge` returns "Pull Request is still a
    /// draft"), so the autonomous-self-merge sensor (#4097) excludes it from the
    /// ready-PR candidate set. Fail-closed: `None` (field absent/unknown from the
    /// listing) is treated as NOT-ready — the sensor admits ONLY `Some(false)`.
    /// Not read by the dashboard panel.
    pub is_draft: Option<bool>,
}

/// One merged-PR summary for the journal's day-scoped "landed changes" table.
/// Sourced from
/// `gh pr list --state merged --search "merged:YYYY-MM-DD" --json number,title,url`.
/// Deliberately minimal: unlike [`OpenPrSummary`] a merged PR has no live gates
/// left to evaluate, so only the fields the journal renders are carried.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MergedPrSummary {
    pub number: u32,
    pub title: String,
    pub url: String,
}

impl OpenPrSummary {
    /// Project this listing summary into a [`PrSnapshot`] so the same
    /// [`evaluate_objective_gates`] used by `merge_pr_if_merge_ready` can
    /// be called against it. `body` and `review_decision` are left empty
    /// because the objective gates do not read them.
    pub fn to_snapshot(&self) -> PrSnapshot {
        PrSnapshot {
            body: String::new(),
            mergeable: self.mergeable.clone(),
            review_decision: String::new(),
            checks: self.checks.clone(),
            base_ref_name: self.base_ref_name.clone(),
            labels: Vec::new(),
            is_draft: self.is_draft,
        }
    }
}

/// Abstract `gh pr` operations used by [`merge_pr_if_merge_ready`]. The trait
/// keeps the evaluation logic testable; production wires it to
/// [`RealPrGhClient`] which shells out to `gh`.
pub trait PrGhClient {
    /// `gh pr view <pr> --repo <repo> --json body,statusCheckRollup,mergeable,reviewDecision,baseRefName`.
    fn view_pr(&self, repo: &str, pr_number: u32) -> SimardResult<PrSnapshot>;
    /// `gh pr merge <pr> --squash --delete-branch --repo <repo>`.
    fn squash_merge(&self, repo: &str, pr_number: u32) -> SimardResult<()>;
    /// `gh pr list --repo <repo> --state open --json number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url --limit <limit>`.
    ///
    /// Added for the operator dashboard's Merge Readiness panel (#1880).
    /// Default impl returns `Ok(vec![])` so existing test fakes that only
    /// exercise the per-PR merge path don't need to grow a stub. The
    /// dashboard handler relies on [`RealPrGhClient`]'s override.
    fn list_open_prs(&self, _repo: &str, _limit: u32) -> SimardResult<Vec<OpenPrSummary>> {
        Ok(Vec::new())
    }

    /// Author-scoped variant of [`list_open_prs`](Self::list_open_prs) for the
    /// autonomous-self-merge sensor (#4097). Lists open PRs authored by
    /// `author` only, pushing the author filter SERVER-SIDE so:
    /// - a busy repo with more than `limit` open PRs can never crowd Simard's
    ///   own eligible PRs out of the fetch window (they'd be silently skipped),
    ///   and
    /// - the transferred + parsed JSON shrinks to just Simard's PRs instead of
    ///   every author's `statusCheckRollup`.
    ///
    /// The default impl delegates to [`list_open_prs`](Self::list_open_prs)
    /// (the caller still applies its own exact-author match as defense-in-
    /// depth), so existing fakes that only script the unscoped listing keep
    /// working unchanged. [`RealPrGhClient`] overrides it to add
    /// `gh pr list --author <author>`.
    fn list_prs_by_author(
        &self,
        repo: &str,
        _author: &str,
        limit: u32,
    ) -> SimardResult<Vec<OpenPrSummary>> {
        self.list_open_prs(repo, limit)
    }

    /// `gh pr list --repo <repo> --state merged --search "merged:<date>" --json number,title,url --limit <limit>`.
    ///
    /// Added for the operator dashboard's Journal tab (#4140): the daily journal
    /// needs the PRs that *merged* on a given day so `merged_pr_count` reflects
    /// real landed changes instead of being structurally zero. `date` is the UTC
    /// calendar day the search is scoped to. Default impl returns `Ok(vec![])`
    /// so existing test fakes that only exercise the per-PR merge path or the
    /// open-PR list don't need to grow a stub; production wires
    /// [`RealPrGhClient`]'s override.
    fn list_merged_prs(
        &self,
        _repo: &str,
        _date: chrono::NaiveDate,
        _limit: u32,
    ) -> SimardResult<Vec<MergedPrSummary>> {
        Ok(Vec::new())
    }

    /// Run an arbitrary POSITIONAL `gh` argv (issue #4097 merge-queue hygiene:
    /// `gh pr comment` / `gh pr close`). The caller builds the argv via the
    /// audited builders in
    /// [`crate::overseer::intervention`], which are structurally incapable of
    /// carrying `--admin` / `--no-verify`. The default fails CLOSED so a fake or
    /// an unwired client performs no mutation; [`RealPrGhClient`] overrides it to
    /// shell out to the `gh` binary (argv-only, never shell-interpolated).
    fn run_gh(&self, _argv: &[String]) -> SimardResult<()> {
        Err(SimardError::MergeAuthorityGhCommandFailed {
            reason: "run_gh not wired on this PrGhClient (fail-closed)".to_string(),
        })
    }

    /// `gh pr close <pr> --repo <repo> --comment <comment>`.
    ///
    /// Added for the overseer's auto-doc-PR reconciliation pass (goal_hygiene):
    /// it closes stale / superseded auto-generated `"Update documentation with
    /// …"` drafts so at most one stays open. The `comment` is authored by the
    /// reconciler (never operator-supplied free text) and explains WHY the PR was
    /// closed (superseded by the canonical PR / stale CONFLICTING draft).
    ///
    /// The default impl is a **no-op** returning `Ok(())` so every existing
    /// fake / unwired client performs NO mutation without needing a stub;
    /// [`RealPrGhClient`] overrides it to shell out to `gh pr close` (argv-only,
    /// never shell-interpolated). A no-op default (rather than the fail-closed
    /// [`run_gh`](Self::run_gh) posture) is safe here because closing is a
    /// hygiene convenience, not a correctness gate — a client that cannot close
    /// simply leaves the duplicates open rather than erroring the cycle.
    fn close_pr(&self, _repo: &str, _pr_number: u32, _comment: &str) -> SimardResult<()> {
        Ok(())
    }
}

/// Max retry attempts for *transient* `gh` read failures (network blips,
/// GitHub 5xx, secondary rate limits). Mutations are deliberately excluded —
/// see [`RealPrGhClient::squash_merge`].
const GH_READ_MAX_RETRIES: u32 = 3;

/// Base backoff (milliseconds) between transient `gh` read retries. Scaled
/// linearly by attempt number so repeated rate-limit hits back off further.
const GH_RETRY_BACKOFF_MS: u64 = 500;

/// Heuristic classifier: should a failed `gh` invocation be retried?
///
/// Returns `true` only for *transient* network / GitHub-availability failures
/// (rate limits, 5xx, connection resets, DNS hiccups, TLS/timeouts) that
/// typically clear after a short backoff. Deterministic failures — auth,
/// not-found, not-mergeable, malformed args, gate refusals — return `false`
/// so they surface immediately instead of looping. Mirrors the substring
/// heuristic the OODA adaptive scaler already uses for 429 detection.
fn is_transient_gh_failure(reason: &str) -> bool {
    const TRANSIENT_NEEDLES: [&str; 14] = [
        "429",
        "rate limit",
        "secondary rate",
        "502",
        "503",
        "504",
        "timed out",
        "timeout",
        "connection reset",
        "could not resolve host",
        "temporary failure",
        "try again",
        "tls handshake",
        "server error",
    ];
    let lower = reason.to_ascii_lowercase();
    TRANSIENT_NEEDLES
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Run an *idempotent* `gh` read closure, retrying transient failures with a
/// bounded linear backoff. Read operations (`gh pr view` / `gh pr list`) carry
/// no side effects, so a retry can never double-apply anything. Deterministic
/// failures and the exhausted-retry case both return the underlying error.
fn retry_transient_gh<T>(op: &str, f: impl FnMut() -> SimardResult<T>) -> SimardResult<T> {
    retry_transient_gh_inner(op, GH_READ_MAX_RETRIES, GH_RETRY_BACKOFF_MS, f)
}

/// Backoff-parameterized core of [`retry_transient_gh`]. Split out so tests can
/// exercise the retry/give-up logic with a zero backoff (no real sleeping).
fn retry_transient_gh_inner<T>(
    op: &str,
    max_retries: u32,
    backoff_ms: u64,
    mut f: impl FnMut() -> SimardResult<T>,
) -> SimardResult<T> {
    let mut attempt = 0u32;
    loop {
        match f() {
            Ok(value) => return Ok(value),
            Err(err) => {
                let transient = matches!(
                    &err,
                    SimardError::MergeAuthorityGhCommandFailed { reason }
                        if is_transient_gh_failure(reason)
                );
                if !transient || attempt >= max_retries {
                    return Err(err);
                }
                attempt += 1;
                let delay = backoff_ms.saturating_mul(u64::from(attempt));
                eprintln!(
                    "[simard] merge-authority: `{op}` transient gh failure \
                     (attempt {attempt}/{max_retries}), backing off {delay}ms: {err}"
                );
                if delay > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                }
            }
        }
    }
}

/// Spawn `gh <args>` and return its stdout on success. `label` is a
/// human-readable rendering of the command used verbatim in error messages so
/// each call site stays a one-liner instead of repeating the
/// spawn → status-check → `MergeAuthorityGhCommandFailed` boilerplate. Both the
/// spawn-failure and non-zero-exit branches return the same error variant the
/// retry classifier inspects, so transient-retry behaviour is unchanged.
fn run_gh_checked(label: &str, args: &[&str]) -> SimardResult<Vec<u8>> {
    let output = std::process::Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| SimardError::MergeAuthorityGhCommandFailed {
            reason: format!("failed to spawn `{label}`: {e}"),
        })?;
    if !output.status.success() {
        return Err(SimardError::MergeAuthorityGhCommandFailed {
            reason: format!(
                "`{label}` exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(output.stdout)
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
        retry_transient_gh("gh pr view", || {
            let pr = pr_number.to_string();
            let stdout = run_gh_checked(
                &format!("gh pr view {pr} --repo {repo}"),
                &[
                    "pr",
                    "view",
                    &pr,
                    "--repo",
                    repo,
                    "--json",
                    "body,statusCheckRollup,mergeable,reviewDecision,baseRefName,labels,isDraft",
                ],
            )?;
            parse_pr_view_json(&stdout)
        })
    }

    /// Squash-merge and delete the head branch. **Single attempt by design:**
    /// unlike the idempotent read paths this is a mutation, so the safe retry
    /// boundary is the gate-revalidating [`merge_pr_if_merge_ready`] cycle —
    /// which re-`view`s the PR and re-checks every gate before any new merge
    /// attempt — not a blind inner loop that could act on stale PR state.
    fn squash_merge(&self, repo: &str, pr_number: u32) -> SimardResult<()> {
        let pr = pr_number.to_string();
        run_gh_checked(
            &format!("gh pr merge {pr} --repo {repo} --squash --delete-branch"),
            &[
                "pr",
                "merge",
                &pr,
                "--repo",
                repo,
                "--squash",
                "--delete-branch",
            ],
        )?;
        Ok(())
    }

    fn list_open_prs(&self, repo: &str, limit: u32) -> SimardResult<Vec<OpenPrSummary>> {
        retry_transient_gh("gh pr list", || {
            let limit_s = limit.to_string();
            let stdout = run_gh_checked(
                &format!("gh pr list --repo {repo} --state open"),
                &[
                    "pr",
                    "list",
                    "--repo",
                    repo,
                    "--state",
                    "open",
                    "--json",
                    "number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url,author,labels,isDraft",
                    "--limit",
                    &limit_s,
                ],
            )?;
            parse_pr_list_json(&stdout)
        })
    }

    fn list_prs_by_author(
        &self,
        repo: &str,
        author: &str,
        limit: u32,
    ) -> SimardResult<Vec<OpenPrSummary>> {
        retry_transient_gh("gh pr list", || {
            let limit_s = limit.to_string();
            let stdout = run_gh_checked(
                &format!("gh pr list --repo {repo} --state open --author {author}"),
                &[
                    "pr",
                    "list",
                    "--repo",
                    repo,
                    "--state",
                    "open",
                    "--author",
                    author,
                    "--json",
                    "number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url,author,labels,isDraft",
                    "--limit",
                    &limit_s,
                ],
            )?;
            parse_pr_list_json(&stdout)
        })
    }

    fn list_merged_prs(
        &self,
        repo: &str,
        date: chrono::NaiveDate,
        limit: u32,
    ) -> SimardResult<Vec<MergedPrSummary>> {
        retry_transient_gh("gh pr list --state merged", || {
            let limit_s = limit.to_string();
            // GitHub search: `merged:YYYY-MM-DD` matches PRs merged on that
            // exact UTC calendar day.
            let search = format!("merged:{}", date.format("%Y-%m-%d"));
            let stdout = run_gh_checked(
                &format!("gh pr list --repo {repo} --state merged --search {search}"),
                &[
                    "pr",
                    "list",
                    "--repo",
                    repo,
                    "--state",
                    "merged",
                    "--search",
                    &search,
                    "--json",
                    "number,title,url",
                    "--limit",
                    &limit_s,
                ],
            )?;
            parse_merged_pr_list_json(&stdout)
        })
    }

    /// Shell out to `gh` with a POSITIONAL argv (never shell-interpolated). Used
    /// for the #4097 merge-queue hygiene mutations (`gh pr comment` / `gh pr
    /// close`), whose argv is built by the audited
    /// [`crate::overseer::intervention`] builders that can never contain
    /// `--admin` / `--no-verify`. Single attempt (a comment/close is a mutation,
    /// like [`squash_merge`](Self::squash_merge)); fail-visible on a non-zero exit.
    fn run_gh(&self, argv: &[String]) -> SimardResult<()> {
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let label = format!("gh {}", refs.join(" "));
        run_gh_checked(&label, &refs)?;
        Ok(())
    }

    /// Close a PR with an explanatory comment. Single attempt (a close is a
    /// mutation, like [`squash_merge`](Self::squash_merge)); fail-visible on a
    /// non-zero exit. Argv is positional / never shell-interpolated.
    fn close_pr(&self, repo: &str, pr_number: u32, comment: &str) -> SimardResult<()> {
        let pr = pr_number.to_string();
        run_gh_checked(
            &format!("gh pr close {pr} --repo {repo}"),
            &["pr", "close", &pr, "--repo", repo, "--comment", comment],
        )?;
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
        #[serde(default)]
        labels: Vec<RawLabel>,
        #[serde(default, rename = "isDraft")]
        is_draft: Option<bool>,
    }
    #[derive(serde::Deserialize)]
    struct RawLabel {
        #[serde(default)]
        name: String,
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
        labels: raw
            .labels
            .into_iter()
            .map(|l| l.name)
            .filter(|n| !n.is_empty())
            .collect(),
        is_draft: raw.is_draft,
    })
}

/// Parse `gh pr list --json number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url`
/// stdout into a vec of [`OpenPrSummary`]. Used by the dashboard's Merge
/// Readiness panel (#1880). Mirrors [`parse_pr_view_json`] for the per-PR
/// listing shape — `gh pr list` returns an array, each element shaped like
/// the `gh pr view` JSON object minus `body`/`reviewDecision`.
pub fn parse_pr_list_json(stdout: &[u8]) -> SimardResult<Vec<OpenPrSummary>> {
    #[derive(serde::Deserialize)]
    struct RawPr {
        #[serde(default)]
        number: u32,
        #[serde(default)]
        title: String,
        #[serde(default, rename = "headRefName")]
        head_ref_name: String,
        #[serde(default, rename = "baseRefName")]
        base_ref_name: String,
        #[serde(default)]
        mergeable: String,
        #[serde(default, rename = "statusCheckRollup")]
        status_check_rollup: Vec<RawCheck>,
        #[serde(default)]
        url: String,
        #[serde(default)]
        author: Option<RawAuthor>,
        #[serde(default)]
        labels: Vec<RawLabel>,
        #[serde(default, rename = "isDraft")]
        is_draft: Option<bool>,
    }
    #[derive(serde::Deserialize)]
    struct RawAuthor {
        #[serde(default)]
        login: String,
    }
    #[derive(serde::Deserialize)]
    struct RawLabel {
        #[serde(default)]
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct RawCheck {
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
    let raws: Vec<RawPr> = serde_json::from_slice(stdout).map_err(|e| {
        SimardError::MergeAuthorityEvaluationFailed {
            reason: format!("could not parse `gh pr list` JSON: {e}"),
        }
    })?;
    Ok(raws
        .into_iter()
        .map(|r| {
            let checks = r
                .status_check_rollup
                .into_iter()
                .map(|c| {
                    let name = c
                        .name
                        .or(c.context)
                        .unwrap_or_else(|| "<unnamed-check>".to_string());
                    let state = match (c.conclusion, c.status, c.state) {
                        (Some(s), _, _) if !s.is_empty() => s,
                        (_, Some(s), _) if !s.is_empty() => s,
                        (_, _, Some(s)) if !s.is_empty() => s,
                        _ => "UNKNOWN".to_string(),
                    };
                    CheckRollupEntry { name, state }
                })
                .collect();
            OpenPrSummary {
                number: r.number,
                title: r.title,
                head_ref_name: r.head_ref_name,
                base_ref_name: r.base_ref_name,
                mergeable: r.mergeable,
                checks,
                url: r.url,
                author: r.author.map(|a| a.login).unwrap_or_default(),
                labels: r
                    .labels
                    .into_iter()
                    .map(|l| l.name)
                    .filter(|n| !n.is_empty())
                    .collect(),
                is_draft: r.is_draft,
            }
        })
        .collect())
}

/// Parse `gh pr list --state merged --json number,title,url` stdout into
/// [`MergedPrSummary`] rows. Mirrors [`parse_pr_list_json`] but for the reduced
/// merged-PR shape the journal needs (#4140). An empty array yields an empty
/// vec (a quiet, no-merge day is honest, not an error).
pub fn parse_merged_pr_list_json(stdout: &[u8]) -> SimardResult<Vec<MergedPrSummary>> {
    #[derive(serde::Deserialize)]
    struct RawMergedPr {
        #[serde(default)]
        number: u32,
        #[serde(default)]
        title: String,
        #[serde(default)]
        url: String,
    }
    let raws: Vec<RawMergedPr> = serde_json::from_slice(stdout).map_err(|e| {
        SimardError::MergeAuthorityEvaluationFailed {
            reason: format!("could not parse `gh pr list --state merged` JSON: {e}"),
        }
    })?;
    Ok(raws
        .into_iter()
        .map(|r| MergedPrSummary {
            number: r.number,
            title: r.title,
            url: r.url,
        })
        .collect())
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
///
/// Made `pub` for #1880 so the operator dashboard's Merge Readiness panel
/// can render the cheap deterministic verdict per open PR without invoking
/// the (expensive) judge per refresh. The dashboard is the only out-of-crate
/// caller; the merge pipeline still uses this internally.
pub fn evaluate_objective_gates(
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
    // Gate 3: not a draft. A draft PR can never be merged server-side, so the
    // rail refuses it deterministically. Fail-closed: an unknown draft state
    // (`None` — `gh` did not report `isDraft`) is treated AS a draft, never
    // waved through. Only an explicit `Some(false)` passes.
    match snapshot.is_draft {
        Some(false) => {}
        Some(true) => {
            return Err(
                "PR is a draft (isDraft=true); a draft can never be merged — mark it ready first"
                    .to_string(),
            );
        }
        None => {
            return Err(
                "PR draft state is unknown (isDraft absent from `gh pr view`); failing closed \
                 (treated as draft) rather than risk merging a draft"
                    .to_string(),
            );
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
    // Creative-idea human-review gate: never auto-merge a PR that carries the
    // block-until-human-review label. The idea-derived PR stays gated (draft +
    // label + owner review requested) until @rysweet approves and clears it.
    // Enforced WITHOUT `--admin`/`--no-verify` — we simply skip the merge.
    if snapshot
        .labels
        .iter()
        .any(|l| l == crate::creative_ideas::CREATIVE_IDEA_PR_LABEL)
    {
        return Ok(MergeOutcome::Refused {
            pr_number,
            reason: format!(
                "PR carries the merge-blocking label `{}` — a creative-idea PR awaiting human review (@{}); skipped.",
                crate::creative_ideas::CREATIVE_IDEA_PR_LABEL,
                crate::creative_ideas::CREATIVE_IDEA_OWNER,
            ),
        });
    }
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
            labels: Vec::new(),
            is_draft: Some(false),
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
        /// Repos passed to `squash_merge`, in call order — lets cross-repo
        /// tests assert the gated authority threads the target repo through to
        /// the underlying `gh pr merge --repo <repo>` rather than a hardcoded
        /// `rysweet/Simard`.
        fn merged_repos(&self) -> Vec<String> {
            self.merge_calls
                .lock()
                .unwrap()
                .iter()
                .map(|(repo, _pr)| repo.clone())
                .collect()
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

        fn kind(&self) -> crate::stewardship::merge_judge::MergeJudgeKind {
            // Tests only need a stable answer; the production code paths under
            // test don't branch on `kind()`. Report `Llm` so `is_configured`
            // returns true, matching the "judge is wired" intent of the
            // fixture.
            crate::stewardship::merge_judge::MergeJudgeKind::Llm
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

    // ─── Creative-idea gate: a block-until-human-review PR is never merged ─

    #[test]
    fn refuses_to_merge_a_creative_idea_pr_awaiting_human_review() {
        // A PR carrying the block-until-human-review label must be SKIPPED by
        // the autonomous merge driver even when every other gate (and the judge)
        // would pass — and without ever invoking the merge command or the judge.
        let gh = FakePrGhClient::new();
        let mut snap = good_snapshot();
        snap.labels = vec![crate::creative_ideas::CREATIVE_IDEA_PR_LABEL.to_string()];
        gh.seed_view(Ok(snap));
        gh.seed_merge(Ok(()));
        let judge = FakeMergeJudge::ready();

        let outcome = run(4242, "rysweet/Simard", &gh, &default_allowlist(), &judge).unwrap();

        match outcome {
            MergeOutcome::Refused { pr_number, reason } => {
                assert_eq!(pr_number, 4242);
                assert!(
                    reason.contains(crate::creative_ideas::CREATIVE_IDEA_PR_LABEL),
                    "refusal must name the blocking label: {reason}"
                );
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        assert_eq!(
            gh.merge_call_count(),
            0,
            "a creative-idea PR must never be squash-merged autonomously"
        );
        assert_eq!(
            judge.call_count(),
            0,
            "the label guard short-circuits before the judge is even consulted"
        );
    }

    #[test]
    fn creative_idea_gate_argv_never_uses_admin_or_no_verify() {
        // The whole creative-idea merge posture forbids privilege bypass: the
        // driver skips the PR (above) and the routing seam's gh argv never carry
        // --admin/--no-verify.
        use crate::creative_ideas::routing::{
            gh_pr_add_label_argv, gh_pr_add_reviewer_argv, gh_pr_draft_argv,
        };
        let argvs = [
            gh_pr_draft_argv("rysweet/Simard", 7),
            gh_pr_add_label_argv(
                "rysweet/Simard",
                7,
                crate::creative_ideas::CREATIVE_IDEA_PR_LABEL,
            ),
            gh_pr_add_reviewer_argv(
                "rysweet/Simard",
                7,
                crate::creative_ideas::CREATIVE_IDEA_OWNER,
            ),
        ];
        for argv in &argvs {
            assert!(!argv.iter().any(|a| a == "--admin"));
            assert!(!argv.iter().any(|a| a == "--no-verify"));
        }
    }

    #[test]
    fn merges_cross_repo_pr_through_the_same_gated_authority() {
        // The gated merge authority must work for ANY repo Simard governs, not
        // only rysweet/Simard, so supply-chain hardening PRs in amplihack-rs,
        // RustyClawd, etc. land through the objective-gates + judge path rather
        // than a bare `gh pr merge`. The target repo is threaded straight
        // through to `gh pr merge --repo <repo>`.
        let cross_repo = "rysweet/amplihack-rs";
        let gh = FakePrGhClient::new();
        gh.seed_view(Ok(good_snapshot()));
        gh.seed_merge(Ok(()));
        let judge = FakeMergeJudge::ready();

        let outcome = run(820, cross_repo, &gh, &default_allowlist(), &judge).unwrap();

        assert_eq!(
            outcome,
            MergeOutcome::Merged {
                pr_number: 820,
                repo: cross_repo.to_string(),
            }
        );
        assert_eq!(gh.merge_call_count(), 1);
        assert_eq!(
            gh.merged_repos(),
            vec![cross_repo.to_string()],
            "the gated authority must squash-merge against the target repo, not a hardcoded rysweet/Simard"
        );
        assert_eq!(judge.call_count(), 1, "judge still gates cross-repo merges");
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

    #[serial_test::serial(cognitive_memory)]
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

    #[serial_test::serial(cognitive_memory)]
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

    #[serial_test::serial(cognitive_memory)]
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

    // ─── #1880 dashboard surface ─────────────────────────────────────────

    /// `evaluate_objective_gates` is now `pub` so the dashboard can call it
    /// without invoking the LLM judge. Verify all three states the panel
    /// renders (ready / not-ready / wrong-base) map to the same verdicts
    /// the merge pipeline would produce — guards against gate drift.
    #[test]
    fn evaluate_objective_gates_pub_surface_matches_merge_pipeline() {
        let allow = default_allowlist();

        // Ready snapshot — all gates pass.
        let ready = good_snapshot();
        assert!(evaluate_objective_gates(&ready, &allow).is_ok());

        // CI-failing snapshot — gate 2 must report the failing check name.
        let mut ci_failing = good_snapshot();
        ci_failing.checks.push(CheckRollupEntry {
            name: "integration-tests".into(),
            state: "FAILURE".into(),
        });
        let err = evaluate_objective_gates(&ci_failing, &allow).unwrap_err();
        assert!(err.contains("integration-tests"), "{err}");
        assert!(err.contains("FAILURE"), "{err}");

        // Wrong-base snapshot — gate 0 must report the base-branch failure
        // first (before mergeable/CI), proving the #1549 ordering invariant.
        let mut wrong_base = good_snapshot();
        wrong_base.base_ref_name = "develop".into();
        wrong_base.mergeable = "CONFLICTING".into(); // would also fail gate 1
        let err = evaluate_objective_gates(&wrong_base, &allow).unwrap_err();
        assert!(
            err.contains("base branch") && err.contains("develop"),
            "wrong-base must surface first; got: {err}"
        );
        assert!(
            !err.contains("CONFLICTING"),
            "wrong-base must short-circuit before the mergeable gate; got: {err}"
        );
    }

    /// `parse_pr_list_json` must accept the `gh pr list` JSON shape and
    /// project it into `OpenPrSummary` rows the dashboard panel can render.
    /// Covers the same conclusion/status/state fall-through as
    /// `parse_pr_view_json` so a check-run mid-flight is reported as
    /// in-progress (the panel maps that to the yellow "pending" badge).
    #[test]
    fn parse_pr_list_json_round_trips_dashboard_shape() {
        let stdout = br#"[
            {
                "number": 1870,
                "title": "feat: agentic merge judge",
                "headRefName": "feat/agentic-merge-judge",
                "baseRefName": "main",
                "mergeable": "MERGEABLE",
                "url": "https://github.com/rysweet/Simard/pull/1870",
                "statusCheckRollup": [
                    { "name": "ci",   "conclusion": "SUCCESS", "status": "COMPLETED" },
                    { "context": "cla/google", "state": "SUCCESS" }
                ]
            },
            {
                "number": 1880,
                "title": "dashboard: surface merge-judge config",
                "headRefName": "feat/merge-readiness-panel",
                "baseRefName": "main",
                "mergeable": "MERGEABLE",
                "url": "https://github.com/rysweet/Simard/pull/1880",
                "statusCheckRollup": [
                    { "name": "build", "status": "IN_PROGRESS", "conclusion": null }
                ]
            }
        ]"#;
        let prs = parse_pr_list_json(stdout).unwrap();
        assert_eq!(prs.len(), 2);

        assert_eq!(prs[0].number, 1870);
        assert_eq!(prs[0].base_ref_name, "main");
        assert_eq!(prs[0].mergeable, "MERGEABLE");
        assert_eq!(prs[0].checks.len(), 2);
        // Conclusion takes precedence over status.
        assert_eq!(prs[0].checks[0].state, "SUCCESS");
        // Falls through to `state` when neither conclusion nor status set.
        assert_eq!(prs[0].checks[1].name, "cla/google");
        assert_eq!(prs[0].checks[1].state, "SUCCESS");

        // Check-run mid-flight: conclusion null → fall through to status.
        assert_eq!(prs[1].checks[0].state, "IN_PROGRESS");

        // `to_snapshot` projects the listing row into the shape
        // `evaluate_objective_gates` consumes.
        let snap = prs[0].to_snapshot();
        assert_eq!(snap.base_ref_name, "main");
        assert_eq!(snap.mergeable, "MERGEABLE");
        assert!(snap.body.is_empty());
    }

    /// `gh pr list` returns `[]` when no PRs are open. Must not panic.
    #[test]
    fn parse_pr_list_json_accepts_empty_array() {
        let prs = parse_pr_list_json(b"[]").unwrap();
        assert!(prs.is_empty());
    }

    /// Issue #4097: the autonomous-self-merge sensor must be able to tell
    /// Simard's OWN PRs from a human's, so `parse_pr_list_json` has to capture
    /// the `author.login` from the `gh pr list --json ...,author` shape.
    #[test]
    fn parse_pr_list_json_captures_author_login() {
        let stdout = br#"[
            {
                "number": 4097,
                "title": "feat: activate autonomous self-merge",
                "headRefName": "feat/self-merge",
                "baseRefName": "main",
                "mergeable": "MERGEABLE",
                "url": "https://github.com/rysweet/Simard/pull/4097",
                "author": { "login": "simard-engineer" },
                "statusCheckRollup": [
                    { "name": "ci", "conclusion": "SUCCESS", "status": "COMPLETED" }
                ]
            }
        ]"#;
        let prs = parse_pr_list_json(stdout).unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(
            prs[0].author, "simard-engineer",
            "author.login must be projected onto OpenPrSummary.author"
        );
    }

    /// A `gh pr list` row missing the `author` object (e.g. a ghost/deleted
    /// account) must default to an empty author, never panic — an empty author
    /// can never equal a configured automerge author, so it fails closed.
    #[test]
    fn parse_pr_list_json_missing_author_defaults_empty() {
        let stdout = br#"[
            {
                "number": 10,
                "title": "orphan",
                "headRefName": "x",
                "baseRefName": "main",
                "mergeable": "MERGEABLE",
                "url": "https://github.com/rysweet/Simard/pull/10",
                "statusCheckRollup": []
            }
        ]"#;
        let prs = parse_pr_list_json(stdout).unwrap();
        assert_eq!(prs.len(), 1);
        assert!(
            prs[0].author.is_empty(),
            "a missing author object must default to empty (fail-closed), not panic"
        );
    }

    /// Issue #4097 (G3): the autonomous-self-merge sensor's engineer-PR gate
    /// needs the PR's labels to tell Simard's OWN engineer PRs from the
    /// operator's own review PRs (both authored by the same login). So
    /// `parse_pr_list_json` must project every `label.name` from the
    /// `gh pr list --json ...,labels` shape onto `OpenPrSummary.labels`.
    #[test]
    fn parse_pr_list_json_captures_labels() {
        let stdout = br#"[
            {
                "number": 4097,
                "title": "feat: activate autonomous self-merge",
                "headRefName": "feat/self-merge",
                "baseRefName": "main",
                "mergeable": "MERGEABLE",
                "url": "https://github.com/rysweet/Simard/pull/4097",
                "author": { "login": "rysweet" },
                "labels": [
                    { "name": "simard-autonomous" },
                    { "name": "enhancement" }
                ],
                "statusCheckRollup": [
                    { "name": "ci", "conclusion": "SUCCESS", "status": "COMPLETED" }
                ]
            }
        ]"#;
        let prs = parse_pr_list_json(stdout).unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(
            prs[0].labels,
            vec!["simard-autonomous".to_string(), "enhancement".to_string()],
            "every label.name must be projected onto OpenPrSummary.labels in order"
        );
    }

    /// A `gh pr list` row with a missing or empty `labels` array must default to
    /// an empty `Vec` — never panic. An engineer PR with no labels then relies on
    /// its branch namespace (the G3 secondary marker); an operator PR with no
    /// labels and a non-engineer branch fails the gate closed.
    #[test]
    fn parse_pr_list_json_missing_labels_defaults_empty() {
        let stdout = br#"[
            {
                "number": 10,
                "title": "no labels here",
                "headRefName": "engineer/10-abcd",
                "baseRefName": "main",
                "mergeable": "MERGEABLE",
                "url": "https://github.com/rysweet/Simard/pull/10",
                "author": { "login": "rysweet" },
                "statusCheckRollup": []
            },
            {
                "number": 11,
                "title": "explicitly empty labels",
                "headRefName": "feat/11",
                "baseRefName": "main",
                "mergeable": "MERGEABLE",
                "url": "https://github.com/rysweet/Simard/pull/11",
                "author": { "login": "rysweet" },
                "labels": [],
                "statusCheckRollup": []
            }
        ]"#;
        let prs = parse_pr_list_json(stdout).unwrap();
        assert_eq!(prs.len(), 2);
        assert!(
            prs[0].labels.is_empty(),
            "a missing labels array must default to an empty Vec (fail-closed), not panic"
        );
        assert!(
            prs[1].labels.is_empty(),
            "an explicitly empty labels array must round-trip to an empty Vec"
        );
    }

    /// Defensive: a `label` object missing its `name` (or with an empty name)
    /// must be dropped, mirroring how `parse_pr_view_json` filters empty label
    /// names — a nameless label can never equal the exact engineer-PR marker.
    #[test]
    fn parse_pr_list_json_drops_nameless_labels() {
        let stdout = br#"[
            {
                "number": 12,
                "title": "malformed label",
                "headRefName": "feat/12",
                "baseRefName": "main",
                "mergeable": "MERGEABLE",
                "url": "https://github.com/rysweet/Simard/pull/12",
                "author": { "login": "rysweet" },
                "labels": [ { "name": "" }, {} , { "name": "simard-autonomous" } ],
                "statusCheckRollup": []
            }
        ]"#;
        let prs = parse_pr_list_json(stdout).unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(
            prs[0].labels,
            vec!["simard-autonomous".to_string()],
            "nameless/empty labels must be dropped, leaving only the real marker"
        );
    }

    /// #4339 root-cause guard: the `gh pr list --json ...,isDraft` boundary must
    /// project `isDraft` onto `OpenPrSummary.is_draft`. A draft row (`true`) and
    /// a ready row (`false`) must round-trip to `Some(true)` / `Some(false)` so
    /// the draft-exclusion gate can key on a KNOWN state. This is the exact
    /// boundary the bug lived at — the field set never fetched `isDraft`.
    #[test]
    fn parse_pr_list_json_captures_is_draft() {
        let stdout = br#"[
            {
                "number": 4336,
                "title": "draft work in progress",
                "headRefName": "engineer/4336-draft",
                "baseRefName": "main",
                "mergeable": "MERGEABLE",
                "url": "https://github.com/rysweet/Simard/pull/4336",
                "author": { "login": "rysweet" },
                "isDraft": true,
                "statusCheckRollup": []
            },
            {
                "number": 4337,
                "title": "ready for merge",
                "headRefName": "engineer/4337-ready",
                "baseRefName": "main",
                "mergeable": "MERGEABLE",
                "url": "https://github.com/rysweet/Simard/pull/4337",
                "author": { "login": "rysweet" },
                "isDraft": false,
                "statusCheckRollup": []
            }
        ]"#;
        let prs = parse_pr_list_json(stdout).unwrap();
        assert_eq!(prs.len(), 2);
        assert_eq!(
            prs[0].is_draft,
            Some(true),
            "a draft row must project isDraft:true onto Some(true)"
        );
        assert_eq!(
            prs[1].is_draft,
            Some(false),
            "a ready row must project isDraft:false onto Some(false)"
        );
    }

    /// #4339 fail-closed at the parse boundary: a `gh pr list` row missing the
    /// `isDraft` field must default to `None` (unknown), NOT `Some(false)`. The
    /// draft gate admits ONLY `Some(false)`, so an unknown draft state is
    /// excluded — never silently treated as ready. Guards against an accidental
    /// `#[serde(default)]`-to-`false` regression.
    #[test]
    fn parse_pr_list_json_missing_is_draft_defaults_none_fail_closed() {
        let stdout = br#"[
            {
                "number": 4338,
                "title": "listing without isDraft field",
                "headRefName": "engineer/4338",
                "baseRefName": "main",
                "mergeable": "MERGEABLE",
                "url": "https://github.com/rysweet/Simard/pull/4338",
                "author": { "login": "rysweet" },
                "statusCheckRollup": []
            }
        ]"#;
        let prs = parse_pr_list_json(stdout).unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(
            prs[0].is_draft, None,
            "a missing isDraft field must default to None (unknown), never Some(false) — \
             the gate then excludes it fail-closed"
        );
    }

    #[test]
    fn parse_merged_pr_list_json_round_trips_journal_shape() {
        // The exact `gh pr list --state merged --json number,title,url` shape.
        let stdout = br#"[
            {"number": 4117, "title": "fix(journal): collapse duplicate dates",
             "url": "https://github.com/rysweet/Simard/pull/4117"},
            {"number": 4122, "title": "fix(dashboard): bound memory growth window",
             "url": "https://github.com/rysweet/Simard/pull/4122"}
        ]"#;
        let merged = parse_merged_pr_list_json(stdout).expect("parses");
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].number, 4117);
        assert_eq!(merged[0].title, "fix(journal): collapse duplicate dates");
        assert_eq!(merged[1].number, 4122);
        assert!(merged[1].url.ends_with("/pull/4122"));
    }

    #[test]
    fn parse_merged_pr_list_json_accepts_empty_array() {
        // A quiet, no-merge day is honest — an empty vec, not an error.
        let merged = parse_merged_pr_list_json(b"[]").unwrap();
        assert!(merged.is_empty());
    }

    #[test]
    fn parse_merged_pr_list_json_rejects_malformed_json() {
        assert!(parse_merged_pr_list_json(b"not json").is_err());
    }

    // ─── Transient gh retry / resilience (Step 8b) ─────────────────────────

    fn gh_failure(reason: &str) -> SimardError {
        SimardError::MergeAuthorityGhCommandFailed {
            reason: reason.to_string(),
        }
    }

    #[test]
    fn transient_classifier_matches_network_and_rate_limit_failures() {
        for reason in [
            "`gh pr view 5 --repo o/r` exited 1: HTTP 429: API rate limit exceeded",
            "GraphQL: something went wrong (502 Bad Gateway)",
            "503 Service Unavailable",
            "error connecting: connection reset by peer",
            "failed to spawn `gh pr view`: dial tcp: i/o timeout",
            "could not resolve host: api.github.com",
            "You have exceeded a secondary rate limit. Please wait a few minutes",
            "net/http: TLS handshake timeout",
        ] {
            assert!(
                is_transient_gh_failure(reason),
                "expected transient classification for: {reason}"
            );
        }
    }

    #[test]
    fn transient_classifier_rejects_deterministic_failures() {
        for reason in [
            "`gh pr view 5` exited 1: GraphQL: Could not resolve to a PullRequest (not found)",
            "Pull request #5 is not mergeable",
            "gh: Not Found (HTTP 404)",
            "authentication required: run `gh auth login`",
            "unknown flag: --repo",
        ] {
            assert!(
                !is_transient_gh_failure(reason),
                "expected deterministic (non-retry) classification for: {reason}"
            );
        }
    }

    #[test]
    fn retry_succeeds_after_transient_then_ok() {
        let calls = Mutex::new(0u32);
        let result: SimardResult<u8> = retry_transient_gh_inner("test", 3, 0, || {
            let mut n = calls.lock().unwrap();
            *n += 1;
            if *n < 3 {
                Err(gh_failure("HTTP 502 Bad Gateway"))
            } else {
                Ok(42)
            }
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(*calls.lock().unwrap(), 3, "should retry twice then succeed");
    }

    #[test]
    fn retry_gives_up_after_exhausting_attempts_on_transient() {
        let calls = Mutex::new(0u32);
        let result: SimardResult<u8> = retry_transient_gh_inner("test", 2, 0, || {
            *calls.lock().unwrap() += 1;
            Err(gh_failure("connection reset by peer"))
        });
        assert!(matches!(
            result,
            Err(SimardError::MergeAuthorityGhCommandFailed { .. })
        ));
        // 1 initial attempt + 2 retries == 3 invocations.
        assert_eq!(*calls.lock().unwrap(), 3);
    }

    #[test]
    fn retry_does_not_retry_deterministic_failures() {
        let calls = Mutex::new(0u32);
        let result: SimardResult<u8> = retry_transient_gh_inner("test", 3, 0, || {
            *calls.lock().unwrap() += 1;
            Err(gh_failure("Pull request is not mergeable"))
        });
        assert!(result.is_err());
        assert_eq!(
            *calls.lock().unwrap(),
            1,
            "deterministic failure must not retry"
        );
    }

    #[test]
    fn retry_does_not_retry_on_immediate_success() {
        let calls = Mutex::new(0u32);
        let result: SimardResult<u8> = retry_transient_gh_inner("test", 3, 0, || {
            *calls.lock().unwrap() += 1;
            Ok(7u8)
        });
        assert_eq!(result.unwrap(), 7);
        assert_eq!(*calls.lock().unwrap(), 1);
    }
}
