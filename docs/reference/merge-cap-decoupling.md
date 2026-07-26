---
title: "Reference: Merge-Cap Decoupling & Bounded Merge Budget"
description: >
  How already-green, CLEAN, MERGEABLE PRs drain even when the per-cycle launch
  cap is exhausted. Covers decoupling VerifyAndMergePr from the launch-cap hold,
  the bounded max_merges_per_cycle budget, the TOCTOU re-verify at merge time,
  and the invariant that merge-eligibility (evaluate_objective_gates) and
  required checks are never bypassed (dedup key delivery:simard_merge_backlog).
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./cross-repo-merge-authority.md
  - ./autonomous-merge-review-gate.md
  - ./ready-prs-sensor-api.md
  - ./draft-pr-exclusion-gate.md
  - ./overseer-tick-details.md
  - ../howto/triage-stale-pull-requests.md
  - ../howto/enable-autonomous-self-merge-canary.md
---

# Reference: Merge-Cap Decoupling & Bounded Merge Budget

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary sources:
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs),
> [`src/overseer/merge_ops.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/merge_ops.rs).
> Tracked by dedup key `delivery:simard_merge_backlog`.

## Overview

Ready-to-merge pull requests were starving. 13 non-draft PRs were
`MERGEABLE` + `CLEAN` with every required check green yet remained unmerged (the
oldest, #4544/#4545, green-and-clean since 2026-07-24). The Overseer's
plan-building short-circuited the cycle with `held: per-cycle launch cap reached`
whenever the launch budget was consumed by cost-bearing launches, and the
`VerifyAndMergePr` interventions never reached `act`.

Merges are now **decoupled** from the launch cap: a green + `CLEAN` + `MERGEABLE`
merge intervention bypasses the launch-cap hold and draws from its **own bounded
merge budget** (`max_merges_per_cycle`). Ready PRs drain even when launches are
capped, at a bounded rate that avoids a thundering herd.

**Eligibility is untouched.** Decoupling affects only *scheduling* (the
launch-cap hold), never *eligibility*: `evaluate_objective_gates` and all
required checks still gate every merge. A `VerifyAndMergePr` path that bypassed
eligibility would be a critical defect.

## What changed

### Launch cap vs. merge budget

`is_cost_bearing` already excludes merge authority — only recipe launches and
audits consume the launch cap:

```rust
// src/overseer/mod.rs
fn is_cost_bearing(iv: &Intervention) -> bool {
    matches!(
        iv,
        Intervention::LaunchRecipe { .. } | Intervention::RunAudit { .. }
    )
}
```

The starvation was **indirect**: plan-building returned
`held_plan(iv, "held: per-cycle launch cap reached")` and short-circuited the
cycle *before* `MergeAuthority` interventions were planned into `act`. The fix
lets green + `CLEAN` + `MERGEABLE` merge interventions **bypass that hold** and
be planned under their own budget.

### `max_merges_per_cycle`

A hard upper bound on auto-merges performed per Overseer cycle. It **defaults to
`2`** — mirroring `max_launches_per_cycle` — to contain blast radius (no
thundering herd) while letting the backlog drain over successive cycles.

| Setting | Type | Default | Purpose |
| --- | --- | --- | --- |
| `max_launches_per_cycle` | `usize` | `2` | Cost-bearing recipe launches / audits per cycle (unchanged) |
| `max_merges_per_cycle` | `usize` | `2` | Auto-merges per cycle, **independent** of the launch cap |

Merges are counted against `max_merges_per_cycle`; launches against
`max_launches_per_cycle`. Exhausting one never starves the other.

### TOCTOU re-verify at merge time

A merge plan built earlier in the cycle could go stale (a PR could stop being
`CLEAN`/`MERGEABLE` between plan and act). Before performing a merge, the act
path **re-verifies** mergeability at merge time; a since-failed PR is not merged.

## Merge-eligibility (unchanged)

Eligibility is decided by the existing objective gate, not by this change:

- non-draft (see [Draft-PR Exclusion Gate](./draft-pr-exclusion-gate.md));
- `MERGEABLE` + `CLEAN`;
- all **required** checks green;
- `evaluate_objective_gates` passes;
- merge authority is enabled for the repo
  (see [Cross-Repo Merge Authority](./cross-repo-merge-authority.md)).

None of these criteria are relaxed. Decoupling only removes the *launch-cap*
scheduling coupling.

## Configuration

`max_merges_per_cycle` defaults to `2` and requires no operator action for the
backlog to drain. Merge authority itself remains gated
by the existing controls documented in
[Cross-Repo Merge Authority](./cross-repo-merge-authority.md) and
[Autonomous Merge Review Gate](./autonomous-merge-review-gate.md).

## Examples

### Ready PRs drain while launches are capped

```text
cycle: launches=2/2  (launch cap EXHAUSTED)
plan : LaunchRecipe … → held: per-cycle launch cap reached
plan : VerifyAndMergePr repo=rysweet/Simard pr=4544 → NOT held (own budget)
act  : re-verify pr=4544 CLEAN+MERGEABLE → merge  (merges=1/N)
act  : re-verify pr=4545 CLEAN+MERGEABLE → merge  (merges=2/N)
```

Before this change:

```text
cycle: launches=2/2
plan : held: per-cycle launch cap reached  → cycle short-circuits
       (VerifyAndMergePr never reaches act; 13 green+clean PRs starve)
```

## Fail-closed / safety guarantees

- **Eligibility never bypassed** — every merge still passes
  `evaluate_objective_gates` and all required checks.
- **Bounded** — `max_merges_per_cycle` caps merges per cycle; no thundering herd.
- **TOCTOU-safe** — mergeability is re-verified at merge time; a since-failed PR
  is not merged.
- **Authority-gated** — merges still require merge authority to be enabled for
  the repo.

## Regression tests

Co-located in
[`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs):

- `ready_prs_drain_when_launch_cap_exhausted` — the pinning test: green + CLEAN +
  MERGEABLE `VerifyAndMergePr` is planned into `act` even with launches at cap.
- `merges_still_require_full_eligibility` — a non-`CLEAN` / failing-check PR is
  never merged; eligibility is unchanged (the `act` re-verify escalates it).
- `max_merges_per_cycle_bound_is_honored` — merges stop at the budget within a
  single cycle and resume next cycle.
- `max_merges_per_cycle_default_is_two` / `merge_budget_is_independent_of_launch_cap`
  — the merge budget defaults to 2 and is a field independent of the launch cap.

## Related

- [Cross-Repo Merge Authority](./cross-repo-merge-authority.md)
- [Autonomous Merge Review Gate](./autonomous-merge-review-gate.md)
- [Ready-PRs Sensor API](./ready-prs-sensor-api.md)
- How-to: [Triage stale pull requests](../howto/triage-stale-pull-requests.md)
