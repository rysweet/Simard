---
title: Autonomous self-merge sensor (ready_prs wire)
description: >
  Why Simard now sees her own merge-ready pull requests — and only her own. The
  thin, deterministic Observe-path sensor that populates ObservedState.ready_prs
  from live open PRs Simard's engineers authored in allowlisted repos, scoped by
  a durable engineer-PR marker (the primary simard-autonomous label OR a secondary
  engineer-exclusive branch namespace) so the operator's own review PRs are NEVER
  auto-merged even when they share the same author login and branch prefixes. The
  already-built
  PrReadyToMerge → DeliveryReady → VerifyAndMergePr path finally activates —
  while the authoritative merge gate stays in merge_authority.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./operational-autonomy-model.md
  - ./enrichment-observability.md
  - ./stewardship-mode.md
  - ../reference/ready-prs-sensor-api.md
  - ../reference/cross-repo-merge-authority.md
  - ../howto/enable-autonomous-self-merge-canary.md
  - ../howto/triage-stale-pull-requests.md
---

# Autonomous self-merge sensor (the `ready_prs` wire)

> **Status: implemented.** This page describes the shipped Observe-path sensor
> in present tense. It fills the last dead wire that kept Simard architecturally
> blind to her own merge-ready pull requests.

Simard's full autonomous-merge path was **built and enabled** long before it
ever fired in production, because the very first sensor in the chain returned
nothing. The path is:

```
ObservedState.ready_prs        (Observe)
        │  for each open PR in an allowlisted repo
        │    ├─ author login == SIMARD_AUTOMERGE_AUTHOR   (whole-login, case-insensitive)
        │    ├─ engineer-PR gate: `simard-autonomous` label (primary) OR engineer-only branch namespace
        │    └─ green + MERGEABLE + base allow-list        (cheap objective pre-filter)
        │  → survivors become candidates
        ▼
Signal::PrReadyToMerge         (src/overseer/signal.rs)
        │
        ▼
ProblemKind::DeliveryReady     (Orient)
        │
        ▼
Intervention::VerifyAndMergePr (Decide — src/overseer/mod.rs)
        │  guardrail allow_verify_merge = true
        ▼
caps.prs.merge() → MergePrOps::merge        (Act — src/overseer/merge_ops.rs)
        │  RecursionGuard admit · verify() checklist · poll-until-green
        ▼
merge_authority::merge_pr_if_merge_ready_with_judge   (the authoritative gate)
        │  objective gates (base + MERGEABLE + CI-green) → agentic merge-judge
        │  (the six merge-ready evidence criteria)
        ▼
gh pr merge <pr> --squash --delete-branch  (NO --admin / NO --no-verify)
```

Every stage after `ready_prs` already existed and was already enabled. The
guardrail `allow_verify_merge` was already `true`
([`with_verify_merge_autonomy(true)`](https://github.com/rysweet/Simard/blob/main/src/overseer/wiring.rs)).
The executor already ran the full merge-ready evaluation. But the production
Observe path hard-coded `ready_prs: Vec::new()`, so `PrReadyToMerge` was **never
emitted**, `DeliveryReady` never triggered, and `VerifyAndMergePr` never fired.
Simard could merge — she just could not **see** what to merge.

This sensor connects the wire.

## What the sensor does

On each acting Overseer cycle, the sensor enriches `ObservedState.ready_prs`
with a **candidate list** of pull requests that are cheaply, deterministically
plausible merge targets:

1. **List** the open pull requests in each **allowlisted** governed repo with the
   author filter pushed **server-side** —
   `gh pr list --author <login> --json number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url,author,labels`
   (`list_prs_by_author`) — the same JSON shape the dashboard's Merge Readiness
   listing uses, now including `labels` for the engineer-PR gate.
2. **Re-check Simard's own author login** in-process by exact, case-insensitive
   whole-login equality against the daemon's OODA/engineer `gh` identity
   (`SIMARD_AUTOMERGE_AUTHOR`) — defense-in-depth over the server-side `--author`
   push. Human logins other than that identity are excluded.
3. **Scope to engineer PRs only** — the safety-critical gate. Simard's engineers
   *and* the operator both author PRs under the same `rysweet` login **and** under
   the same common branch prefixes (`feat/`, `fix/`, `chore/`), so neither the
   author login nor a shared branch prefix can tell an engineer's merge-ready PR
   from the operator's own review PR. A PR is a candidate **only if** it carries
   the durable engineer marker:
   - the `simard-autonomous` **label** — the **primary** marker, applied by the
     engineer at PR-create time; the only marker that works on the shared branch
     prefixes both parties use, **OR**
   - an **engineer-exclusive branch namespace** — the **secondary** marker,
     limited to `engineer/` (the engineer worktree) and `chore/advisory-` (the
     supply-chain steward), both assigned deterministically in Rust and never used
     by operator review PRs.

   A PR with **neither** the label **nor** an engineer-exclusive branch namespace
   is **never** a candidate — even if the author matches and CI is green. In
   particular, an operator review PR on a shared `feat/…` or `fix/…` branch with no
   `simard-autonomous` label is excluded. This is what keeps the operator's own
   review PRs out of the autonomous loop.
4. **Cheap pre-filter** to green + `MERGEABLE` using the already-fetched
   `statusCheckRollup` + `mergeable` fields (the same deterministic
   `evaluate_objective_gates` the authoritative gate uses for its objective
   pass). No merge-judge runs here.
5. **Populate** `ObservedState.ready_prs` with the survivors.

The surviving candidates flow into the existing signal → orient → decide → act
chain, where the **authoritative** gate does the real work.

## The load-bearing boundary: candidate list vs. merge truth

This sensor is a **thin deterministic rail** that produces a *candidate list*.
It is deliberately **not** the merge gate. The single source of merge truth
stays where it already lives — in
[`merge_authority`](../reference/cross-repo-merge-authority.md):

| | Sensor (this page) | Authoritative gate (`merge_authority`) |
|---|---|---|
| **Role** | Lists plausible candidates | Decides whether to merge |
| **When** | Observe pass, every cycle | Act pass, per selected PR |
| **Checks** | Author + engineer-PR marker (label OR engineer-exclusive branch namespace) + base + `MERGEABLE` + CI-green (cheap, from the list JSON) | Objective gates + the agentic merge-judge (the six merge-ready evidence criteria) |
| **Merges?** | **Never** — returns candidates only | Yes: `gh pr merge --squash --delete-branch` |
| **Fail mode** | Fail-visible → empty list | Fail-closed → refuse |

The sensor's pre-filter is **additive strictness**: it can only *remove*
candidates the authoritative gate would also reject. It can never *add* a merge
the gate would refuse, and it never weakens the gate. If the sensor and the gate
ever disagreed, the gate wins — the sensor cannot merge anything.

## Safety posture

This is a high-blast-radius capability, so it ships **OFF by default** and
fails closed and visible at every step.

- **Allowlist, default empty, fail-closed.** Autonomous merge is gated behind an
  explicit repo allowlist (`SIMARD_AUTOMERGE_REPOS`). **Unset or empty ⇒ zero
  candidates ⇒ no autonomous merge.** Deploying this code does **not** merge
  anything until an operator canary-enables a specific repo. See
  [Enable autonomous self-merge (canary)](../howto/enable-autonomous-self-merge-canary.md).
- **Own PRs only.** Whole-login, case-insensitive equality against Simard's
  configured OODA/engineer identity (`SIMARD_AUTOMERGE_AUTHOR`, required — no
  ambient `gh` fallback). This identity is **distinct** from the
  `simard-overseer[bot]` identity that the `RecursionGuard` refuses, so the two
  never collide and valid candidates survive the guard. An unset or unmatched
  author ⇒ empty list.
- **Engineer PRs only — the operator's own PRs are never touched.** The author
  filter alone is not enough: Simard's engineers *and* the operator both author
  under the same `rysweet` login and share the common branch prefixes (`feat/`,
  `fix/`, `chore/`). So after the author filter, a mandatory **engineer-PR gate**
  requires a durable self-identifying marker — the `simard-autonomous` label
  (`SIMARD_ENGINEER_PR_LABEL`) **OR** an engineer-exclusive branch namespace
  (`engineer/`, `chore/advisory-`). A PR that carries **neither** is excluded even
  when its author matches and CI is green. An operator review PR on a shared
  `feat/…` or `fix/…` branch with no `simard-autonomous` label matches neither
  arm, so it is **never** an auto-merge candidate. The `simard-autonomous` label
  is the **primary** marker — the only one that works on the shared prefixes both
  parties use, so every engineer PR must carry it (the engineer applies it at
  `gh pr create` time). The engineer-exclusive branch namespace is a **secondary**,
  defense-in-depth marker (assigned deterministically in Rust by the engineer
  worktree and the supply-chain steward, and never used by operator PRs), catching
  an engineer PR even if the label was forgotten.
- **Authoritative gate unchanged.** The authoritative gate in `merge_authority`
  stays fail-closed (`Unclear`/error ⇒ `NotReady` ⇒ refuse) and still **never**
  uses `--admin` or `--no-verify`. Branch protections are never bypassed.
- **Evidence-gated by the merge-judge.** The merge-judge prompt
  (`prompt_assets/simard/merge_readiness_judge.md`) requires all six substantive
  merge-ready evidence sections in the PR body — QA-team, Documentation,
  Quality-audit, CI, Scope, and Verdict — before a `ready` verdict, so autonomous
  merges clear the same evidence bar as human-driven ones.
- **No wall-clock timeouts.** The rail is poll/deterministic only.
- **Fail-visible.** Any survey/`gh`/parse error surfaces via `tracing::warn!`
  and yields an empty candidate list — never a silent wrong merge.

## Where it lives in the loop

Population happens in the acting `run_cycle` **enrichment path**
([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)),
alongside the other enriched Observe fields (`blocked_goals`, `workstream_gaps`,
`recent_step_failures`, `recall`). It is **not** populated in
`observed_from_snapshot`, which stays a side-effect-free projection of the
read-only status snapshot (its unit tests assert empty IO fields). See
[enrichment observability](./enrichment-observability.md) for the general
pattern this follows.

For the exact API — the survey seam, the config resolvers, and the data flow —
see the [`ready_prs` sensor API reference](../reference/ready-prs-sensor-api.md).
For the operator runbook to turn it on one repo at a time, see
[Enable autonomous self-merge (canary)](../howto/enable-autonomous-self-merge-canary.md).
