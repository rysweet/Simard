---
title: CI-Health Sweep — Governed-Fleet Reference
description: Reference for simard::ci_health and the `simard ci-health` subcommand — the codified, reproducible governed-fleet CI-health sweep that classifies each default-branch workflow as green, actionable_failure, or ignored(reason).
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./stewardship-api.md
  - ../concepts/stewardship-mode.md
  - ./cross-repo-merge-authority.md
  - ../../src/ci_health/mod.rs
  - ../../src/ci_health/cache.rs
  - ../../src/operator_cli/ci_health.rs
---

# CI-Health Sweep — Governed-Fleet Reference

Module: `simard::ci_health`
Source: `src/ci_health/`
CLI: `simard ci-health`

The CI-health sweep is a precise, reproducible check of whether every **active**
default-branch workflow across the amplihack ecosystem (Simard + its governed
sibling repos) is green. It exists so the standing *CI-health stewardship* goal
produces verifiable, re-runnable evidence instead of hand-rolled `gh run list`
claims.

## Why a codified sweep

A naive `gh run list --branch main` sweep cannot distinguish an **actionable
active-CI failure** from a **non-actionable non-green signal**. Two real cases
made that ambiguity concrete:

- **azlin** has seven agentic scheduled workflows (Code Quality Tracker, CI/CD
  Workflow Health Monitor, …) whose *latest* default-branch run is `failure` —
  but every one is `disabled_manually`. A disabled workflow cannot run again, so
  its stale failure is not active CI.
- **agent-kgpacks** "Build Knowledge Pack" latest run is `cancelled`/`skipped` —
  a non-failure conclusion, not a broken build.

A sweep that reads only run conclusions either (a) mislabels those as failures,
or (b) glosses over them and claims "every workflow is `success`" — which is
literally false for those latest runs. Carrying the workflow *enablement state*
alongside the run conclusion is what lets the sweep be both correct and honest.

## Classification

For each workflow, the sweep reads its enablement state and the latest run on
the repo's default branch, then classifies it:

| Verdict | Condition |
|---|---|
| `green` | workflow **active** and latest run concluded `success` |
| `actionable_failure` | workflow **active** and latest run concluded `failure`, `timed_out`, or `startup_failure` |
| `ignored` | any of: workflow disabled (`workflow_disabled`); non-failure conclusion such as cancelled/skipped/neutral/action_required/stale (`non_failure_conclusion:<c>`); no default-branch run (`no_default_branch_run`); run not completed (`run_in_progress`) |

The **fleet is green iff it contains zero actionable failures.** Disabled
workflows, cancelled/skipped runs, and in-progress runs never fail the fleet;
an active workflow whose latest run genuinely failed always does.

Classification ([`classify::build_report`]) is a total, pure function over the
[`FleetSnapshot`] — no I/O, no `gh`, no clock — so the verdict is deterministic
and exhaustively unit-tested.

### Transient-infra resilience (keeping green builds green)

The sweep reads each workflow's **latest** default-branch run, so any conclusion
it sees is what the steward acts on. That makes it sensitive to *transient
GitHub-infrastructure* failures — a run whose real gates (fmt, clippy, tests)
all pass but that GitHub marks `failure` because a non-gate infra step flaked.
The recurring example is the artifact service returning
`Failed to CreateArtifact: Request timeout` after `actions/upload-artifact`'s
own five internal retries during a brownout, which fails the whole `verify`
workflow even though the code is green. A single sweep landing inside that
window would classify a self-healing blip as an `actionable_failure` and file a
spurious tracking issue.

Because the classifier is deliberately a pure function of the run conclusion
(it does not scrape job logs), this class is mitigated **at the workflow level**
rather than in classification: `.github/workflows/verify.yml` makes its artifact
steps non-fatal so an artifact-service outage cannot turn an all-green run red.

- The **diagnostic** `cargo-test log` upload is `continue-on-error` — nothing
  consumes it, so its upload timing out must never fail the build.
- The **binary** upload is `continue-on-error`, and its only consumer, the
  `e2e-dashboard` job, falls back to rebuilding the binary (from the read-only
  shared cargo cache) when the artifact download misses. A skipped upload
  therefore self-heals within the same run instead of reddening the default
  branch.

The remedy for a genuine, non-transient default-branch failure is unchanged: it
is an `actionable_failure`, turns the fleet red, and is routed to a
deduplicated tracking issue by `--file-issues` — which also **closes** that
issue once the workflow is green again (see [Closing tracking issues when a
workflow recovers](#closing-tracking-issues-when-a-workflow-recovers---file-issues)).

## Module layout

```
src/ci_health/
├── mod.rs        public entrypoint, GOVERNED_REPOS, sweep_live/sweep_fixture/report_to_json, run_sweep
├── types.rs      WorkflowState, RunConclusion, WorkflowRun/Snapshot, RepoSnapshot (head_sha, green_from_cache), FleetSnapshot
├── classify.rs   WorkflowVerdict, IgnoreReason, build_report, repo_cacheable, update_cache_from_report, FleetReport (serializable DTOs)
├── cache.rs      GreenShaCache — persisted {repo -> last-known-green head SHA}
├── gh.rs         GhWorkflowClient trait (incl. head_sha), RealGhWorkflowClient, pure parse/join helpers, fixture loader
├── diagnose.rs   RunDiagnostics trait + RealGhRunDiagnostics, parse_run_diagnosis, RunDiagnosis/FailedJob (root-cause of a failing run)
├── report.rs     render_human
├── steward.rs    actionable-failure -> deduplicated-issue steward (ci_signature_for/ci_failure_signature, file_issues_for_report) + green-again resolution (CiIssueResolver, resolve_issues_for_report)
└── tests.rs      unit tests
```

## Last-known-green head-SHA cache

Re-reading every workflow and its latest run for all ten `GOVERNED_REPOS` on
every cycle is wasteful when the fleet is already green and unchanged — the
churn loop the standing CI-health goal kept falling into. To break it, the sweep
caches, per repo, the default-branch **head commit SHA** at which the repo was
last verified green (`gh api repos/<owner>/<repo>/commits/<default> --jq .sha`),
persisted as JSON at `<state_root>/state/ci_health_green_sha.json`.

On the next sweep, [`collect_fleet`] resolves each repo's default branch and head
SHA (two cheap `gh` calls) and, when the head SHA equals the cached green SHA,
**skips** the expensive per-workflow collection entirely. The repo is emitted as
`green_from_cache` (its `RepoReport.green_from_cache` is `true` and its workflow
list is empty), keeping the fleet green while advertising that it was not
re-collected. `FleetReport.repos_from_cache` counts how many repos were served
this way; the human report prints a `[cache]` line for each.

### Why a SHA is a sound skip key

A repo is only ever *recorded* in the cache when it is green **and**
[`classify::repo_cacheable`] holds: **no active workflow demonstrates that it can
run without a new default-branch commit**. A disabled workflow is ignored (it
cannot run); a commit-driven latest run (`push`, `pull_request`,
`pull_request_target`, `merge_group`) is the intended green case; and an active
workflow that has **never run** on the default branch is allowed, because a
scheduled trigger would already have produced runs — a never-run workflow is
almost certainly triggered only by events that need a commit or explicit human
action (PR, tag, `release`, a human-invoked `workflow_dispatch`, Copilot agents).

A repo is **disqualified** (always freshly swept, never cached) if any active
workflow's latest run is still in progress, or **completed with a
non-commit-driven event** (`schedule`, `workflow_dispatch`, `repository_dispatch`,
`dynamic`/Dependabot, `issues`, …) — such a workflow has *demonstrably* run
without a commit, so a future such run could fail on an unchanged head SHA. This
is why the fleet's scheduled/agentic repos (e.g. azlin's `Security Scanning`,
Simard's `advisory-scan`, RustyClawd's `Claude Code Sync Monitor`) are never
served from cache.

A commit-driven workflow cannot produce a new run without a new default-branch
commit, which necessarily changes the head SHA and misses the cache. So for a
cached repo, an unchanged head SHA means no active workflow has demonstrably run
since the green verdict — the verdict still holds. The one residual (a scheduled
workflow so newly added it has never fired) is narrow, self-heals on the next
commit, and is covered by `--no-cache` / periodic full sweeps.

[`classify::update_cache_from_report`] is the sole cache writer: it keeps the
entry for a cache-served repo, records the head SHA for a freshly-green cacheable
repo, and invalidates the entry for any repo that fails or is no longer
cacheable.

The cache is a pure optimization: a missing file (first run) or an
unreadable/corrupt/out-of-version file degrades to a full sweep (the correct,
complete behavior), never to a wrong verdict.

## `simard ci-health`

```
simard ci-health [--json] [--no-cache] [--file-issues] [--exit-zero] [--from-json <path>]

  --json               Emit the FleetReport as JSON (default: human table).
  --no-cache           Force a full re-collection of every repo, ignoring the
                       last-known-green head-SHA cache (the cache is still
                       refreshed from this sweep). Alias: --refresh.
  --file-issues        For each distinct actionable failure, file a
                       deduplicated tracking issue in the failing repo, and
                       close any open tracking issue whose workflow is green
                       again (see below). Read-only by default; this flag opts
                       in to the writes. Rejected with --from-json.
  --exit-zero          Exit 0 even on a red fleet, as long as the sweep itself
                       ran without an operational error. For the unattended
                       scheduled sweep (see below); an actual gh/parse error
                       still exits non-zero.
  --from-json <path>   Classify an offline snapshot fixture instead of calling
                       `gh` (the fixture shape mirrors the live snapshot).
```

- Without `--from-json`, the sweep reads live GitHub state via `gh` for every
  slug in [`ci_health::GOVERNED_REPOS`]: the repo's default branch
  (`gh repo view`), its default-branch head commit SHA
  (`gh api repos/<owner>/<repo>/commits/<default> --jq .sha`, the cache key),
  workflow states + ids (`gh workflow list --json name,state,id`), and
  the latest default-branch run per workflow
  (`gh run list --branch <default> --json workflowName,workflowDatabaseId,status,conclusion,event,createdAt,databaseId`).
  A repo whose head SHA matches its cached last-known-green SHA is served from
  cache (see [above](#last-known-green-head-sha-cache)) and its workflow reads
  are skipped; `--no-cache` forces the full reads.
  Runs are matched to workflows by the unique `workflowDatabaseId`, not the
  (non-unique) display name, so two workflow files sharing a `name:` never
  collapse onto one run.
  Because that branch-wide run list is windowed, any **active** workflow with no
  run inside the window is queried directly (`gh run list --workflow <id> --limit 1`)
  so a stale failing run of an infrequently-triggered workflow can never be
  silently dropped and reported as green.
- **Exit code** follows the verdict: `0` when the fleet is green, non-zero when
  any actionable failure exists (mirrors `simard self-health`). `--exit-zero`
  overrides only that verdict — a red fleet still exits `0` — for the unattended
  [scheduled sweep](#scheduled-recurring-sweep); an operational error (a failed
  `gh`/parse) is surfaced *before* the verdict, so `--exit-zero` never masks a
  broken sweep, only a truthfully-reported red fleet.

The human report leads with a greppable banner (`CI-HEALTH: GREEN` /
`CI-HEALTH: FAILING`) and a per-repo breakdown; each actionable failure is
hoisted to the top with a direct run URL. The `--json` report is the same data
as a stable `FleetReport` object.

### Filing deduplicated tracking issues (`--file-issues`)

Detecting failures is only half of the standing CI-health stewardship goal; the
other half is *"dedupe to one issue/PR per distinct failure."* The
[`ci_health::steward`] module (`src/ci_health/steward.rs`) converts a
[`FleetReport`]'s actionable failures into deduplicated GitHub issues, reusing
the [Stewardship](./stewardship-api.md) dedup contract rather than forking it:

- **Distinct-failure identity.** A distinct CI failure is one broken *workflow
  on a repo*, keyed by `<repo> :: <workflow>`. The volatile run id/URL and the
  specific failing conclusion (`failure` / `timed_out` / `startup_failure`) are
  **excluded** from the signature, so the same broken workflow hashes
  identically across sweeps and across a `failure`↔`timed_out` flap — yielding
  exactly one issue per broken workflow. The signature is
  `failure_signature("ci_workflow_failure", "<repo> :: <workflow>")`, the same
  8-byte SHA-256 prefix the orchestrator-failure steward uses.
- **Target repo is the failing repo itself.** Unlike orchestrator-failure
  routing, a CI failure's repo is already known (it is a governed repo), so no
  routing matrix is consulted — the issue is filed in the repo whose CI failed.
- **Dedup, then file.** For each distinct signature the steward searches the
  target repo (`gh issue list -R <repo> --state open --search
  "stewardship-signature:<sig> in:body"`). A match short-circuits to
  `MatchedExisting` (no new issue); otherwise a new issue is filed with the
  standard `filed-by: simard-stewardship` / `stewardship-signature: <sig>`
  front-matter plus CI-health specifics (repo, workflow, default branch, latest
  conclusion, run URL). Two failures that hash to the same signature in one
  sweep collapse to a single issue.
- **Resilient to search-index lag.** GitHub's issue *search* index is
  eventually consistent — a tracking issue filed seconds or minutes ago may not
  be searchable yet, so two sweeps inside that window would each see an empty
  search and file a duplicate. The dedup search
  ([`RealGhClient::search_issues`]) defeats this: when the full-text search does
  not already surface the signed issue, it complements the hits with a
  **strongly-consistent** scan of the newest open issues
  (`gh issue list --state open --limit <N>`, no `--search`) and dedups against
  the union. This is what makes "exactly one issue per broken workflow" hold
  across back-to-back sweeps, not just across well-separated ones.
- **Fail-loud.** A `gh` error on the search propagates and **no** issue is filed
  for that signature — the loop never assumes "no matches" on a degraded search,
  matching the orchestrator steward's contract.

#### Root-cause diagnosis in the issue body

Tracking *that* a workflow broke is not enough to act on it — the goal's third
clause is *"diagnose root cause."* So every **newly-filed** issue embeds a
`## Root cause` block pinpointing which of the failing run's job(s) and step(s)
failed, read from `gh run view <run_id> --json jobs` by [`ci_health::diagnose`]
(`src/ci_health/diagnose.rs`). This localizes the failure — a human or a
downstream `ci-diagnostic` fixer sees *which job and step* failed without
hunting through the run — and links the run for the failing logs, an on-ramp to
*"launch a fix."*

- **Structured, not log-scraped.** Diagnosis reads the jobs API, whose
  `jobs[].conclusion` / `jobs[].steps[].conclusion` name the failing job and
  step directly. The set of "failing" conclusions is exactly the sweep's
  actionable set (`failure` / `timed_out` / `startup_failure`), so `cancelled` /
  `skipped` / `success` steps are never mistaken for the root cause. A failing
  job with no individually-failing step (e.g. a `timed_out` job) is rendered
  with its own reported conclusion rather than a guessed cause.
- **Best-effort, never blocks tracking.** Filing the tracking issue is the
  correctness-critical act; a diagnosis that cannot be fetched (a `gh` error, a
  malformed jobs response, or a failure whose run id was not captured) must not
  abort it. The block then records *why* it is unavailable and links the run —
  no silent degradation.
- **Only for genuinely-new issues.** Diagnosis is fetched solely on the
  file-new path (after the dedup search found no existing issue), so a re-swept,
  already-tracked failure and a green fleet both cost **zero** extra `gh` calls.
  A consequence is that an existing tracked issue is not retroactively
  re-diagnosed; the block reflects the run that first tripped the sweep.

`--file-issues` is **opt-in**: the default sweep is read-only. It requires a
live sweep and is rejected when combined with `--from-json` (filing real issues
from an offline fixture would be wrong). The exit code still follows the verdict
(non-zero while any actionable failure exists); the filed/matched issues are
printed after the report.

#### Closing tracking issues when a workflow recovers (`--file-issues`)

A tracking issue promises, in its own body, to track a broken workflow *"until
its default-branch CI is green again."* Filing without a matching **close**
would leave that promise unkept: a workflow that failed, got a tracking issue,
and later went green would keep a stale open issue forever, violating the goal's
*"one issue/PR per distinct failure"* hygiene. So the `--file-issues` write is
bidirectional. In the same pass, [`resolve_issues_for_report`] closes the
tracking issue of every workflow that is **green again**:

- **Keyed on the same signature.** Filing and resolution share one signature
  helper, [`ci_signature_for`] (`<repo> :: <workflow>`), so the issue a green
  workflow resolves is exactly the one its earlier failure filed — proven by a
  signature-parity unit test. Each freshly-collected repo that has **any** green
  workflow has its open issues listed **once** via the REST issue-list endpoint
  (`gh issue list -R <repo> --state open --limit <N> --json
  number,url,title,body`), filtered **locally** to this steward's tracking
  issues by their unique `ci-health-workflow:` body marker, and each green
  workflow is then matched against that filtered list by signature. A hit is
  closed with `gh issue close --reason completed` and a **green-evidence
  comment** that links the now-green run (or names the default-branch run
  generically when the run id was not captured).
- **Why not a `--search` pre-filter.** GitHub's issue *search* tokenizes
  `ci-health-workflow` into separate words, so `--search "ci-health-workflow
  in:body"` both returns unrelated issues that merely mention those words and,
  worse, can push a real tracking issue past its result window on a busy repo
  (e.g. a governed repo with hundreds of open issues) — silently failing to
  close it. Listing on the core REST endpoint and matching the exact marker
  in-process is truncation-safe up to `<N>` open issues per repo (sized well
  above governed-repo volumes) and also avoids the Search API's ~30/min
  secondary rate limit.
- **O(repos), not O(green-workflows).** Resolution costs one issue-list request
  per freshly-collected repo (a repo with no green workflow, or no open tracking
  issue, does no per-workflow work), so a healthy fleet is reconciled in a
  handful of core-API `gh` calls — rather than one Search-API call per green
  workflow.
- **Files before it resolves.** Within `--file-issues`, filing (the
  correctness-critical path — a genuinely-broken workflow must get a tracking
  issue) runs first, then resolution, so a resolution `gh` error can never
  starve filing.
- **One close per shared issue.** Two workflow files sharing a `name:` hash to
  one signature, so filing opens a single issue for the pair; resolution tracks
  the issue numbers it has closed this repo so such a pair closes that one issue
  exactly once (no duplicate comment or spurious already-closed error).
- **Conservative — only `green` resolves, and never over a live failure.** A
  workflow verdict of exactly `green` closes an issue. A **still-failing**
  workflow (its issue stays open, and is instead matched/re-filed by the filing
  half) and every **ignored** signal — an in-progress rerun, a disabled
  workflow, a cancelled/skipped run, a never-run workflow — never close a
  tracking issue. In particular, an in-flight rerun of a previously-broken
  workflow keeps its issue open until it *concludes* green, so a
  red→(rerunning)→green transition never closes prematurely. A green workflow
  whose signature still has a **live actionable failure** this sweep (a
  same-`name:` sibling file is broken and collapses to the same signature/issue)
  is also skipped — keyed on the same `actionable_failures` set filing uses — so
  a green sibling never closes the issue that is still tracking its broken twin
  (which would otherwise flap the issue closed then re-opened next sweep).
- **Cache-aware.** A repo served from the last-known-green SHA cache carries no
  workflow list this sweep, so its issues are resolved on the next full
  (`--no-cache`) sweep or the next time a commit re-collects it. In practice the
  failing→green transition *always* re-collects the repo (its cache entry was
  invalidated while it was failing), so resolution fires exactly at the
  transition; a steadily-green cached repo has no open issue left to close.
- **Fail-loud.** A `gh` error on either the list or the close propagates — a
  degraded list never silently resolves nothing, and a failed close is never
  mistaken for a resolved issue. Resolution runs **even when the fleet is green**,
  because a green fleet can still carry stale issues from a since-recovered
  failure; the closed issues are printed after the report alongside any
  filed/matched ones.

### Governed fleet

`GOVERNED_REPOS` is the source of truth in code for the swept slugs; it mirrors
the ecosystem table in `prompt_assets/simard/engineer_system.md` (note
`amplihack` → `amplihack-rs` on GitHub).

## Scheduled recurring sweep

Detection + filing only catches a regression when *something runs the sweep*.
Running it by hand each cycle is exactly the un-evidenced, human-in-the-loop
process this steward exists to replace, so the sweep also runs unattended on a
cadence via **`.github/workflows/ci-health.yml`** — the CI-health analogue of
the supply-chain steward's `advisory-scan.yml`.

- **Trigger.** `schedule` (daily, `17 5 * * *` UTC — offset from advisory-scan's
  06:00 so the two stewards don't contend for a runner) plus `workflow_dispatch`
  for on-demand/manual sweeps. It never runs on `push`/`pull_request`, so it is
  fully decoupled from PR gating and can never block unrelated work.
- **Command.** `simard ci-health --no-cache --file-issues --exit-zero`:
  - `--no-cache` re-audits every repo each run (no green-SHA skips), so a
    regression is caught the same day it lands rather than on the next
    cache-invalidating change.
  - `--file-issues` is the human-free alarm: each distinct actionable failure
    becomes one deduplicated tracking issue (with a root-cause block) in the
    failing repo, and recovered workflows' issues are closed.
  - `--exit-zero` keeps the *run itself* green on a red fleet. The alarm is the
    filed tracking issue, not a red run — and if this scheduled run went red on
    a sibling's failure, the next sweep would classify Simard's own `ci-health`
    workflow as a fresh actionable failure and file a tracking issue for it, a
    self-referential loop. An actual `gh`/parse error still fails the run.
- **Auth.** `GH_TOKEN` is `secrets.STEWARD_GH_TOKEN` falling back to the
  workflow's `github.token`. Cross-repo issue writes (a sibling repo's failure
  files an issue in *that* repo) need a token with fleet-wide `issues:write`;
  the default token is scoped to this repo only. With the bot token absent, the
  green path and Simard's own issues still work, and a sibling failure surfaces
  as a fail-loud `gh issue create` error (a visible red run) rather than a
  silently-dropped failure — fail-safe, not fail-open, matching advisory-scan.
- **Concurrency.** A `ci-health` concurrency group (no cancel-in-progress) means
  two runs never race on the same tracking issues.
- **Build cache.** The sweep is a Rust binary, so the job must build `simard`
  before it can audit the fleet. It restores the **same** warm cargo cache that
  `verify.yml` populates on `main` (Swatinem `rust-cache` with
  `shared-key: simard-ci-v2`, `save-if: false` — read-only, so the scheduled
  sweep never poisons the shared PR/verify cache). Without that explicit shared
  key rust-cache derives a per-job key from the job name (`sweep`), a namespace
  nothing ever writes to; every scheduled sweep was then a guaranteed cache
  miss that rebuilt the whole workspace from scratch and overran the 20-minute
  job budget before the sweep binary even started. Sharing verify's cache turns
  the cold rebuild into a few-minute incremental one, mirroring verify.yml's
  read-only fallback-build job.

## Reproducing a captured sweep

The offline path makes any sweep reproducible without network access: capture a
snapshot fixture in the shape under `tests/gadugi/fixtures/ci-health-*.json`,
then `simard ci-health --from-json <fixture>`. The gadugi scenario
`tests/gadugi/ci-health-sweep.yaml` uses two committed fixtures to assert that
disabled/cancelled/in-progress signals stay green while a genuine active-CI
failure turns the fleet red.

## Related

- Concept: [Goal Stewardship Mode](../concepts/stewardship-mode.md)
- [Stewardship API](./stewardship-api.md) — orchestrator-failure → issue routing
- [Cross-Repo Merge Authority](./cross-repo-merge-authority.md)
- Source: `src/ci_health/`, `src/operator_cli/ci_health.rs`,
  `.github/workflows/ci-health.yml` (scheduled runner)
