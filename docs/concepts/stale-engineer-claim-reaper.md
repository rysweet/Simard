---
title: "Stale-Engineer-Claim Reaper (independent within-incarnation claim reclaim)"
description: >
  How Simard closes the within-incarnation engineer-claim leak. The admission
  ledger's engineer_claims table (cap 24) can accumulate orphaned claims whose
  goal is never polled again and whose sentinel PID is the still-alive daemon —
  so neither the collision-reclaim path nor the per-goal heartbeat cleanup ever
  frees them until a full daemon restart. A periodic reaper on the Overseer tick
  sweeps ALL claims independently of per-goal polling and reclaims those whose
  engineer is provably dead (no worktree, or newest-file mtime stale beyond a
  generous threshold). Fail-closed, fail-visible, no wall-clock kill of a live
  engineer.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ./engineer-claim-liveness-lease.md
  - ./investigate-stale-engineer-before-reap.md
  - ../reference/claim-reaper-api.md
  - ../reference/engineer-claim-release-api.md
  - ../reference/engineer-worktree-sweep-safety.md
  - ../operations/claim-reaper-kill-switch.md
  - ../howto/diagnose-leaked-engineer-claims.md
---

# Stale-Engineer-Claim Reaper

> **Status: implemented.** This page describes shipped behaviour in present
> tense. The reaper lives in
> [`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs)
> and runs synchronously on the Overseer tick
> ([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
> `run_cycle`, beside `reconcile_inflight_investigations`). It reuses the
> [`release_engineer_claim`](../reference/engineer-claim-release-api.md) ledger
> chokepoint and the guarded worktree-removal primitive.

## The leak this reaper closes

The [engineer-claim liveness lease](./engineer-claim-liveness-lease.md) makes a
`engineer_claims` row releasable and reclaimable, and it fully covers the
**cross-incarnation** case (host reboot, SIGKILL, OOM): the next spawn attempt
for that goal reclaims the dead claim on a `claim_key` `PRIMARY KEY` collision.

But there is a **within-incarnation** leak the lease does not cover. The
`engineer_claims` table is capped at 24 rows. Inside a single daemon
incarnation, orphaned claims accumulate and permanently consume a cap slot until
the daemon restarts. Both existing reclaim paths are **inert** for these
orphans:

| Existing path | Where | Why it never fires for these orphans |
|---|---|---|
| Liveness-gated reclaim-on-collision | `record_action` in [`ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs) | Only fires when a **new** claim collides on the same `claim_key`. If the goal is never spawned again (completed, removed, or test junk like `g1`/`test-goal`), no collision ever happens. And its liveness check reads the worktree sentinel PID, which stores `std::process::id()` = the **daemon** PID ([`engineer_worktree/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/engineer_worktree/mod.rs)) — always alive within one incarnation, so `is_claim_live` returns `true`. |
| Heartbeat-stale cleanup | `advance_goal_with_subordinate` in [`subordinate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/subordinate.rs) | Only runs when that **specific goal is polled** in the Decide/coverage set. An orphaned claim whose goal has left the coverage set is never polled, so cleanup never reaches it. |

**Live evidence (2026-07-15):** 24/24 claim slots held, but only ~12
engineer-worktrees were touched in the last 10 minutes; ~12 worktree dirs were
~4 days old; and 5 claim rows (`g1`, `g2`, `test-goal`, `goal-fix-docs`,
`goal-improve-tests`) had **zero** backing worktree **and** zero backing
process — pure leaks holding cap slots hostage until restart.

> **The gap:** there was **no sweep that reclaims claims independently of
> per-goal polling.** The reaper is exactly that additive, independent sweep.

## What the reaper does

On every Overseer tick, the reaper enumerates **all** `engineer_claims` rows
via [`list_engineer_claims()`](../reference/claim-reaper-api.md) and, for each
claim, asks a liveness probe for a verdict. A claim is reclaimed when the
engineer is **provably dead**:

| Verdict | Condition | Action |
|---|---|---|
| `Dead { reason: NoWorktree }` | No engineer-worktree directory maps to the claim's `goal_id` — there is nothing to protect. | **Reclaim immediately.** |
| `Dead { reason: HeartbeatStale, age }` | A worktree exists but its **newest-file mtime** is older than the stale threshold (default 30 min, env-tunable). | **Reclaim.** |
| `Live` | Worktree exists and its newest-file mtime is fresh (within threshold). | **Skip** — the engineer is (or may be) working. |
| `Live` (fail-closed) | The worktree root is momentarily unreadable, or any I/O error prevents proving death. | **Skip** — uncertainty is never treated as death. |

Reclaim reuses the existing machinery — it does **not** hand-roll a second
`DELETE` or a bespoke removal:

1. **Ledger row** → [`release_engineer_claim(claim_key)`](../reference/engineer-claim-release-api.md)
   (the single idempotent, fail-visible `DELETE` chokepoint).
2. **Orphaned worktree directory** → the same guarded removal primitive used by
   the engineer sweep (`assert_under_root` + `remove_dir_all`), so
   [worktree-reaping safety guards](../reference/engineer-worktree-sweep-safety.md)
   still apply.

## Why worktree mtime, not `check_heartbeat`

The per-goal heartbeat check (`check_heartbeat`) derives age from
`progress.heartbeat_epoch` in hive memory, keyed by **`agent_name`**. A sweep
that enumerates by `claim_key` alone has no `agent_name` and cannot call it.

The reaper therefore uses the **newest-file mtime under the engineer-worktree
directory** as its idle-staleness signal. This is **not a second heartbeat
format** — the engineer's own file writes *are* the liveness signal, and mtime
reads them without parsing any bespoke file. It preserves the exact
idle-since-newest-activity notion the acceptance tests assert.

## Binding guarantees

- **Fail-closed.** A claim is reclaimed only on *positive proof of death*
  (absent worktree, or a fresh-mtime read that is provably older than the
  threshold). A **fresh** worktree is never reaped. Any scan/IO uncertainty
  resolves to `Live` → skip.
- **No wall-clock kill.** This is idle-staleness detection, not a run-duration
  cap. A busy engineer whose files keep changing keeps its claim **no matter how
  long it runs** — matching the operator rule that agentic steps must never be
  killed by elapsed time. The generous 30-minute default protects a
  live-but-quiet engineer (a long compile/test).
- **Fail-visible.** Every reclaim emits exactly one `[simard]` tracing line
  naming the `claim_key`, the staleness `age` (or `n/a` for a missing worktree),
  and the `reason` (`no-worktree` | `heartbeat-stale`). Reclaims are never
  silent.
- **Reuse, don't duplicate.** The ledger `DELETE` and the guarded worktree
  removal are the same chokepoints used everywhere else. No `--admin`, no
  hand-rolled SQL.
- **Additive and independent.** The cap value (24), the admission gate, and the
  collision-reclaim logic are unchanged. The reaper is a purely additive sweep.
  Pre-existing leaked rows and stale dirs self-heal on the reaper's first run —
  there is no one-off migration.

## Cross-repo correctness

`claim_key = "{owner}/{repo}:{goal_id}"` and claims span repositories
(`rysweet/Simard`, `rysweet/agent-kgpacks-rs-audit`, `rysweet/amplihack-rs`).
The reaper recovers `goal_id` with `split_once(':')` — the segment after the
first `:` — reusing the exact split already used by the admission gate's
`is_claim_live` (`src/ooda_actions/advance_goal/typed_goal_session.rs:490`).
Since `owner`, `repo`, and `goal_id` never contain a `:`, splitting on the
first or the last colon yields the same `goal_id`; the reaper follows existing
code precedent rather than duplicating a second, divergent split. It then
matches the worktree directory whose
`goal_id_from_worktree_dir(dir) == goal_id`. Because
`goal_id` is unique within a single daemon's `state_root`, one scan of
`<state_root>/engineer-worktrees/` covers **all** repositories — the match is
repo-agnostic. The delete target is always a directory **discovered on disk**,
never a path constructed from `claim_key`, so a corrupt key can only fail to
match — it can never become a deletion target.

## Where it runs

The reaper is a **synchronous** step inside `Overseer::run_cycle`, beside
`reconcile_inflight_investigations` — the natural home for a periodic reconcile.
There is **no new background thread and no independent loop**. Its cadence is the
Overseer tick cadence.

## Invariants at a glance

| Situation | Behaviour |
|---|---|
| Claim with no backing worktree | **Reclaimed** immediately (`no-worktree`) |
| Claim with fresh worktree (mtime ~now) | **Kept** (engineer may be working) |
| Claim with stale worktree (mtime > threshold) | **Reclaimed** (`heartbeat-stale`, with age) |
| Worktree root unreadable / any I/O error | **Kept** (fail-closed) |
| Reaper disabled (`SIMARD_CLAIM_REAP_ENABLED` falsey) | **No reclaims**, even for a no-worktree claim |
| Reclaim path | `release_engineer_claim` + guarded worktree removal (no hand-rolled SQL) |

## Related

- [Engineer-Claim Liveness Lease](./engineer-claim-liveness-lease.md) — the
  release-on-termination + collision-reclaim lease this reaper complements.
- [Investigate-Before-Reap](./investigate-stale-engineer-before-reap.md) — how the
  `HeartbeatStale` branch now investigates + preserves evidence before any reclaim.
- [Claim-Reaper API](../reference/claim-reaper-api.md) — the sweep function,
  liveness probe seam, cleanup seam, and config resolvers.
- [Engineer-Claim Release & Reclaim API](../reference/engineer-claim-release-api.md)
- [Worktree Reaping Safety Guards](../reference/engineer-worktree-sweep-safety.md)
- [Claim-Reaper Kill Switch & Tuning](../operations/claim-reaper-kill-switch.md)
- [Diagnose leaked engineer claims](../howto/diagnose-leaked-engineer-claims.md)
