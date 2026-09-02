---
title: "Worker-Presence Liveness Verification (fail-open reclaim close)"
description: >
  How the OODA per-goal reasoner decides whether a goal already has a worker.
  `worker_present` is now a verified-live-process fact, not bare membership in
  the in-memory `engineer_worktrees` map. A leaked or idle worktree claim no
  longer reads as "present forever", so the existing stale-claim reclaim signal
  can fire and the goal is unwedged. This is the fail-OPEN counterpart to the
  fail-CLOSE engineer-claim liveness lease (#4437 / #4608 / #4574).
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ../reference/ooda-per-goal-cycle-api.md
  - ../reference/worker-presence-liveness-api.md
  - ./engineer-claim-liveness-lease.md
  - ./agentic-per-goal-per-cycle.md
  - ./stale-engineer-claim-reaper.md
  - ../fail-open-audit.md
---

# Worker-Presence Liveness Verification

> **Status: implemented.** This page describes shipped behaviour in present
> tense. The presence read lives in `gather_per_goal_cycle_ctx` in
> [`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs);
> the liveness verifier it delegates to is
> [`find_live_engineer_for_goal`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs).

## What `worker_present` means

Every cycle, for every active goal, the per-goal reasoner is handed a
[`PerGoalCycleCtx`](../reference/ooda-per-goal-cycle-api.md). One of its facts is:

```rust
pub worker_present: bool,   // a *verified-live* engineer exists for this goal right now
```

The reasoner uses `worker_present` to distinguish "work is genuinely in flight,
leave it alone" (`Continue` / `Wait`) from "no live work, start the next piece"
(`Spawn`). It also gates the companion `stale_claim_secs` reclaim signal: that
signal is populated **only when a worker is expected but not present**.

Because `worker_present` steers whether a goal ever spawns again, its
correctness is load-bearing. If it is wrong in the "present" direction, the goal
is silently starved.

## The bug this fixes (#4631): fail-OPEN presence

Historically the presence read was pure map membership:

```rust
// BEFORE — fail-open
let worker_present = state.engineer_worktrees.contains_key(goal_id);
```

`engineer_worktrees` is an **in-memory** map of worktree claims. An entry lands
there when an engineer is dispatched. But an entry can **outlive** the engineer
that created it:

- the engineer process is SIGKILLed, OOM-killed, or dies in the host reboot;
- the daemon crashes and reloads with a stale claim;
- a worktree is leaked (allocated, never cleanly released).

In every one of those cases the map still contains the `goal_id`, so
`contains_key` returned `true` and the reasoner was told *"worker present …
alive"* **forever**. The goal therefore:

- never took the `Spawn` branch (a worker "already exists"), and
- never populated `stale_claim_secs` (the reclaim input fires only when a worker
  is *expected but absent*), so the reclaim path never re-engaged.

The result is a **fail-open** leak: a dead or leaked engineer is never
reclaimed, and the goal makes no further progress. This is the mirror image of
the #4437 fail-**close** defect (a claim wrongly *blocking* a needed spawn) that
was hardened in PRs #4608 and #4574 — the same class of "trust an in-memory
claim over real process liveness" bug, pointed the other way.

## The fix: verify a live process, not a map entry

`worker_present` is now gated by an authoritative filesystem liveness check:

```rust
// AFTER — verified-live, fail-closed on "present"
let worker_present = state.engineer_worktrees.contains_key(goal_id)
    && crate::ooda_actions::advance_goal::find_live_engineer_for_goal(
        &crate::goal_curation::simard_state_root(),
        goal_id,
    )
    .is_some();
```

The change is deliberately minimal and **reuses the exact hardening already
shipped for the fail-close counterpart** (Approach B — mirror #4608 / #4574
rather than approximate them). No new liveness logic is introduced, no new
public API is exposed, and the diff is confined to `cycle.rs`.

- `contains_key(goal_id)` is kept as a cheap **short-circuit**: the filesystem
  scan only runs for goals that actually have a map entry, so the added IO is
  one worktrees-root scan per active-with-claim goal per cycle. Note the
  aggregate is quadratic — *N* claim-holding goals each scan the full
  worktrees root (~*N* entries) — but *N* is bounded small by concurrent-engineer
  admission limits, so this is acceptable (see the [API reference](../reference/worker-presence-liveness-api.md#cost)).
- [`find_live_engineer_for_goal`](../reference/worker-presence-liveness-api.md)
  scans `<state_root>/engineer-worktrees/{goal_id}-*/.simard-engineer-claim`
  sentinels and confirms the recorded PID is alive **with a process
  start-time guard** so a recycled PID (an unrelated new process that reuses the
  old PID) is correctly treated as *dead*.

When the worktree entry exists but no live, start-time-verified engineer is
found, `worker_present` is now `false`. The reasoner then sees a goal that
*expects* a worker but has none, `stale_claim_secs` is populated, and the
**existing** reclaim path re-engages automatically. No new threshold, config, or
reap trigger is added.

```
engineer_worktrees has goal_id?
   ├── no  ── worker_present = false            (unchanged; short-circuit)
   └── yes ── find_live_engineer_for_goal(...)?
                ├── Some(live worktree) ── worker_present = true   (genuinely working, untouched)
                └── None (dead/leaked)  ── worker_present = false  (reclaimable — bug fixed)
```

## Fail-closed direction

Note the asymmetry, which is intentional and matches the fail-close lease:

> **A goal is only reported as *having a live worker* when a live worker is
> actually proven.** Anything short of positive proof of a live, start-time-
> verified engineer resolves to `worker_present = false`.

`find_live_engineer_for_goal` returns `None` both when it *proves* no live
engineer exists **and** when it hits a transient `read_dir`/IO error. For the
`worker_present` read this is the **safe** direction: an ambiguous signal
reports "no live worker", which at worst causes the reasoner to re-examine (via
`stale_claim_secs` → `Investigate`) a goal whose worker may still be alive. It
never *fabricates* a live worker. Destructive reclaim is still reached only as a
reasoned follow-up (see
[Agentic Per-Goal, Per-Cycle Decision](./agentic-per-goal-per-cycle.md)), so a
brief false "not present" cannot itself kill a live engineer — the reasoner
inspects before acting.

## Why there is no wall-clock timeout

As with the fail-close lease, presence is decided on **PID liveness**, never on
elapsed time:

> **A genuinely-working engineer is `worker_present == true` no matter how long
> it has been running.** Only a *dead* or *leaked* claim reads as not present.

The starttime guard inherited from `find_live_engineer_for_goal` is the control
that keeps a slow-but-alive engineer present while catching PID reuse. No
`SIMARD_*_SECS` threshold participates in the presence decision; the pre-existing
`STALE_SECS` survives only as the value that *populates* `stale_claim_secs` once
`worker_present` is already `false`.

## Scope: only the reasoner presence read changed

This fix touches **one** site — the `worker_present` computation in
`gather_per_goal_cycle_ctx`. It does **not** change any other consumer of
`engineer_worktrees`:

| Consumer | File | Unchanged behaviour |
|---|---|---|
| Reap victim selection | `subordinate.rs` | still selects by map membership |
| `has_tombstoned_engineer` | `subordinate.rs` | unchanged |
| Resource admission | `resource_admission` | unchanged |
| Stale-claim reaper | `overseer/claim_reaper.rs` | unchanged |

The fail-close SQLite admission gate (#4437 / #4608 / #4574) is likewise
untouched — it already verifies liveness. This change makes the **reasoner's**
presence read consistent with it.

## Invariants at a glance

| Situation | `worker_present` |
|---|---|
| No `engineer_worktrees` entry for goal | `false` (short-circuit) |
| Entry exists, sentinel PID alive + starttime matches | `true` |
| Entry exists, sentinel PID dead | `false` (reclaimable) |
| Entry exists, sentinel PID recycled (starttime mismatch) | `false` (reclaimable) |
| Entry exists, sentinel missing / unparseable | `false` (reclaimable) |
| Entry exists, worktree dir unreadable (transient IO) | `false` (safe direction; reasoner re-examines, never fabricates a worker) |

## Related

- Reference / API: [Worker-Presence Liveness API](../reference/worker-presence-liveness-api.md)
- [Reference: OODA Per-Goal-Cycle API](../reference/ooda-per-goal-cycle-api.md) — where `worker_present` is consumed
- [Engineer-Claim Liveness Lease](./engineer-claim-liveness-lease.md) — the fail-CLOSE counterpart
- [Agentic Per-Goal, Per-Cycle Decision](./agentic-per-goal-per-cycle.md)
- [Fail-Open Audit](../fail-open-audit.md)
