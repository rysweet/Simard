---
title: CI-Health Sweep — Governed-Fleet Reference
description: Reference for simard::ci_health and the `simard ci-health` subcommand — the codified, reproducible governed-fleet CI-health sweep that classifies each default-branch workflow as green, actionable_failure, or ignored(reason).
last_updated: 2026-07-16
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
deduplicated tracking issue by `--file-issues`.

## Module layout

```
src/ci_health/
├── mod.rs        public entrypoint, GOVERNED_REPOS, sweep_live/sweep_fixture/report_to_json, run_sweep
├── types.rs      WorkflowState, RunConclusion, WorkflowRun/Snapshot, RepoSnapshot (head_sha, green_from_cache), FleetSnapshot
├── classify.rs   WorkflowVerdict, IgnoreReason, build_report, repo_cacheable, update_cache_from_report, FleetReport (serializable DTOs)
├── cache.rs      GreenShaCache — persisted {repo -> last-known-green head SHA}
├── gh.rs         GhWorkflowClient trait (incl. head_sha), RealGhWorkflowClient, pure parse/join helpers, fixture loader
├── report.rs     render_human
├── steward.rs    actionable-failure -> deduplicated-issue steward (ci_failure_signature, file_issues_for_report)
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
simard ci-health [--json] [--no-cache] [--file-issues] [--from-json <path>]

  --json               Emit the FleetReport as JSON (default: human table).
  --no-cache           Force a full re-collection of every repo, ignoring the
                       last-known-green head-SHA cache (the cache is still
                       refreshed from this sweep). Alias: --refresh.
  --file-issues        For each distinct actionable failure, file a
                       deduplicated tracking issue in the failing repo
                       (see below). Read-only by default; this flag opts in to
                       the write. Rejected with --from-json.
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
  any actionable failure exists (mirrors `simard self-health`).

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

`--file-issues` is **opt-in**: the default sweep is read-only. It requires a
live sweep and is rejected when combined with `--from-json` (filing real issues
from an offline fixture would be wrong). The exit code still follows the verdict
(non-zero while any actionable failure exists); the filed/matched issues are
printed after the report.

### Governed fleet

`GOVERNED_REPOS` is the source of truth in code for the swept slugs; it mirrors
the ecosystem table in `prompt_assets/simard/engineer_system.md` (note
`amplihack` → `amplihack-rs` on GitHub).

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
- Source: `src/ci_health/`, `src/operator_cli/ci_health.rs`
