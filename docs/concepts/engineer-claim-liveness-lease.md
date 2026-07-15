---
title: "Engineer-Claim Liveness Lease (release-on-termination + stale reclaim)"
description: >
  How Simard's engineer-admission gate stays consistent with the *real*
  liveness of an engineer process. An engineer claim is now a releasable lease:
  it blocks a duplicate spawn only while a real engineer is alive, and is freed
  the instant that engineer terminates (success, failure, blocked, crash, or
  zombie-reap). Covers the invariant, the two mechanisms (deterministic
  release-on-termination and liveness-verified stale reclaim), and why there is
  no wall-clock timeout.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ../reference/engineer-claim-release-api.md
  - ../architecture/engineer-agent-orchestration.md
  - ../reference/ooda-engineer-lifecycle-recipe.md
  - ../reference/ooda-capability-api.md
  - ../operations/engineer-admission-kill-switch.md
---

# Engineer-Claim Liveness Lease

> **Status: implemented.** This page describes shipped behaviour in present
> tense. The gate lives in
> [`src/typed_ooda/ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs);
> the `engineer_claims` table is declared in
> [`src/typed_ooda/schema.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/schema.rs);
> the release call fires from the engineer-termination path in
> [`src/ooda_actions/advance_goal/subordinate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/subordinate.rs).

## What an engineer claim is

When the OODA Act phase spawns an engineer for a goal, the typed-OODA ledger
records a **single-active-claim** row in the `engineer_claims` table. The claim
key is deterministic:

```
claim_key = "{owner}/{repo}:{goal_id}"      # e.g. rysweet/Simard:harden-backups
```

The claim exists to enforce one invariant:

> **At most one live engineer per goal.** While an engineer is actually running
> for a goal, a second concurrent spawn for that same goal is rejected with
> `AdmissionRejected` — no duplicate work.

## The bug this design fixes

Historically the claim was **append-only**: `claim_key` was a `PRIMARY KEY`,
the row had no timestamp / lease / expiry, and there was **no `DELETE` path
anywhere in the codebase**. Once a goal spawned its first engineer, the row
persisted for the lifetime of the store and permanently rejected every future
spawn for that goal — even long after the engineer had terminated (including
engineers reaped as zombies). The Act phase would then report `no_action` every
cycle with reasons like:

> *"No new engineer spawned this cycle: an engineer claim is already active for
> rysweet/Simard:&lt;goal&gt;. … the single-active-claim policy rejected it
> because an in-flight engineer is already advancing this goal."*

…while in reality **no** engineer was in flight. Goals silently stopped making
progress after exactly one spawn.

## The fix: a claim is a liveness lease, not a tombstone

An engineer claim is now a **releasable lease keyed to the actual liveness of
the engineer process**. A goal is blocked from a *duplicate* spawn only while a
real engineer is alive, and the claim is freed the moment that engineer
terminates — through **any** exit path. Two mechanisms deliver this, and they
are deliberately layered (primary + safety net):

### 1. Release-on-termination (deterministic, primary)

When an engineer terminates and its lifecycle is cleaned up, Simard **deletes
the `engineer_claims` row by `claim_key`**. This is a correctness invariant, not
a reasoning task — it is fully deterministic and runs on **every** termination
path:

| Termination path | Claim released? |
|---|---|
| Clean success (terminal outcome recorded) | ✅ |
| Failure | ✅ |
| Blocked | ✅ |
| Crash | ✅ |
| Zombie / reap (PID gone, no clean exit) | ✅ |

The release is wired into the single deterministic chokepoint that **all**
termination paths pass through —
`cleanup_engineer_worktree_for_goal(state, goal_id)` in
[`subordinate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/subordinate.rs)
(invoked from **six call sites** that map onto the five outcome categories
above) — co-located with the drop of the `.simard-engineer-claim` worktree
sentinel.
It reconstructs the `claim_key` from `{owner}/{repo}:{goal_id}` and calls the
idempotent [`release_engineer_claim`](../reference/engineer-claim-release-api.md)
capability. Deleting a claim that is already gone is a **success** (0 rows
affected → `Ok(())`), so double-release and release-before-insert races are safe.

### 2. Stale-claim reclaim (deterministic safety net)

Release-on-termination is primary, but a process can die in a way that never
runs cleanup (SIGKILL, host reboot, OOM). To guarantee no goal is *ever*
permanently wedged, the admission gate **verifies liveness before rejecting**:

When `record_action` tries to insert a new claim and hits the `claim_key`
`PRIMARY KEY` constraint, it does **not** immediately reject. Instead it asks
the authoritative liveness signal whether the *existing* claim corresponds to a
**live** engineer:

- **Claim is live** → keep the rejection (`AdmissionRejected`). The
  single-active-claim invariant is preserved; the running engineer keeps its
  claim.
- **Claim is dead / orphaned** → **reclaim**: delete the stale row and retry the
  insert **once**, inside the same spawn transaction (no zero-claim window). The
  new spawn proceeds.

The liveness source of truth is **process liveness of the engineer's sentinel
PID**, reusing the exact primitives already used elsewhere in the daemon:

- `find_live_engineer_for_goal(state_root, goal_id)`
  ([`spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs))
  scans `<state_root>/engineer-worktrees/*/.simard-engineer-claim` sentinel
  files, and
- `is_pid_alive_public(pid)`
  ([`engineer_worktree/claim.rs`](https://github.com/rysweet/Simard/blob/main/src/engineer_worktree/claim.rs))
  confirms the PID is actually alive — with a **process start-time guard** so a
  recycled PID (a new, unrelated process that happens to reuse the old PID) is
  correctly treated as *dead*.

This is the same signal already used by
`count_live_engineer_claims`
([`ooda_brain/context.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/context.rs));
the SQLite admission gate is now **consistent** with it.

> **Fail-closed when liveness is uncertain.** Reclaim requires *proof of death*
> — a successful sentinel scan that finds no matching, alive, start-time-verified
> PID. Anything short of that proof (for example a transient inability to read
> the worktree directory) is treated as **live**, so an ambiguous signal never
> reclaims a claim out from under a running engineer.

## Why there is no wall-clock timeout

Reclaim fires **only on provably-dead liveness** (sentinel PID absent, or a
start-time mismatch proving PID recycling). It never uses elapsed time to
invalidate a claim.

> **A genuinely-working engineer is never reclaimed, no matter how long it
> runs.** This is intentional and matches the operator rule that agentic steps
> must never be killed by elapsed time. A slow-but-alive engineer keeps its
> claim; only a *dead* claim is freed.

If a lease column is ever added for extra robustness it would model the existing
`effect_jobs` heartbeat/expiry pattern — but **PID liveness of the sentinel
remains the authoritative signal**, and no expiry may kill a live engineer. The
shipped design needs no such column: release-on-termination plus
liveness-verified reclaim fully cover correctness, so `schema.rs` is unchanged
and existing stores need **no migration**. Any rows leaked before the fix
self-heal — they are reclaimed the next time their goal attempts a spawn.

## Invariants at a glance

| Situation | Behaviour |
|---|---|
| Engineer alive for goal | Duplicate concurrent spawn **rejected** (`AdmissionRejected`) |
| Engineer terminated (any path) | Claim **released** deterministically at cleanup |
| Claim exists but engineer PID is dead / recycled | Claim **reclaimed** on next spawn attempt |
| Liveness cannot be proven dead (scan/IO error) | Treated as **live** → duplicate spawn stays **rejected** (fail-closed) |
| Release DELETE affects 0 rows | **Success** (idempotent) |
| Release / reclaim SQL error | **Surfaced** (logged + `Err`), never swallowed |

## Related

- Reference / API: [Engineer-Claim Release & Reclaim API](../reference/engineer-claim-release-api.md)
- [Engineer-Agent Orchestration](../architecture/engineer-agent-orchestration.md)
- [OODA Engineer-Lifecycle Recipe](../reference/ooda-engineer-lifecycle-recipe.md)
- [Engineer-Admission Kill Switch](../operations/engineer-admission-kill-switch.md)
