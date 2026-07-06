---
title: CI-Health Sweep — Governed-Fleet Reference
description: Reference for simard::ci_health and the `simard ci-health` subcommand — the codified, reproducible governed-fleet CI-health sweep that classifies each default-branch workflow as green, actionable_failure, or ignored(reason).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./stewardship-api.md
  - ../concepts/stewardship-mode.md
  - ./cross-repo-merge-authority.md
  - ../../src/ci_health/mod.rs
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

## Module layout

```
src/ci_health/
├── mod.rs        public entrypoint, GOVERNED_REPOS, sweep_live/sweep_fixture/report_to_json
├── types.rs      WorkflowState, RunConclusion, WorkflowRun/Snapshot, RepoSnapshot, FleetSnapshot
├── classify.rs   WorkflowVerdict, IgnoreReason, build_report, FleetReport (serializable DTOs)
├── gh.rs         GhWorkflowClient trait, RealGhWorkflowClient, pure parse/join helpers, fixture loader
├── report.rs     render_human
└── tests.rs      unit tests
```

## `simard ci-health`

```
simard ci-health [--json] [--from-json <path>]

  --json               Emit the FleetReport as JSON (default: human table).
  --from-json <path>   Classify an offline snapshot fixture instead of calling
                       `gh` (the fixture shape mirrors the live snapshot).
```

- Without `--from-json`, the sweep reads live GitHub state via `gh` for every
  slug in [`ci_health::GOVERNED_REPOS`]: the repo's default branch
  (`gh repo view`), workflow states (`gh workflow list --json name,state`), and
  the latest default-branch run per workflow
  (`gh run list --branch <default> --json workflowName,status,conclusion,event,createdAt,databaseId`).
- **Exit code** follows the verdict: `0` when the fleet is green, non-zero when
  any actionable failure exists (mirrors `simard self-health`).

The human report leads with a greppable banner (`CI-HEALTH: GREEN` /
`CI-HEALTH: FAILING`) and a per-repo breakdown; each actionable failure is
hoisted to the top with a direct run URL. The `--json` report is the same data
as a stable `FleetReport` object.

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
