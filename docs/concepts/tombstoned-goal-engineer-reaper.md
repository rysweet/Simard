---
title: "Tombstoned-Goal Engineer Reaper (reap in-flight engineers whose goal was removed/completed)"
description: >
  How Simard stops an already-dispatched engineer from running to completion —
  and producing unwanted PRs — after its goal has been removed or completed by
  the operator. Each OODA cycle, right after the goal board is reloaded and
  tombstone-filtered, a reconciliation compares the persistent
  OodaState.engineer_worktrees map against the durable tombstone set and reaps
  every in-flight engineer whose goal is genuinely GONE (removed/completed).
  The reap gracefully terminates the tracked subordinate (SIGTERM, never
  kill -9) and cleans up its worktree through the existing chokepoints. State-
  driven and tombstone-gated — never a wall-clock timeout, never a reap of a
  healthy engineer whose goal is still on the board.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ./stale-engineer-claim-reaper.md
  - ./goal-board-persistence.md
  - ../reference/tombstoned-goal-engineer-reaper-api.md
  - ../reference/engineer-worktree-isolation.md
  - ../reference/engineer-claim-release-api.md
  - ../reference/subagent-tmux-tracking.md
  - ../howto/diagnose-a-reaped-engineer-after-goal-removal.md
---

# Tombstoned-Goal Engineer Reaper

> **Status: implemented.** This page describes shipped behaviour in present
> tense. The reconciliation function lives in
> [`src/ooda_actions/advance_goal/subordinate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/subordinate.rs)
> (`reap_engineers_for_tombstoned_goals`) and is invoked once per OODA cycle
> from the daemon loop in
> [`src/operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs),
> **within** the board reload / `filter_tombstoned` reconciliation block (where
> the block-scoped `cycle_tombstones` set is in scope). It reuses the
> existing graceful `kill_subordinate` primitive and the existing
> `cleanup_engineer_worktree_for_goal` chokepoint — no new process-killer.

## The leak this reaper closes

When an operator runs `simard goal remove <id>` or `simard goal complete <id>`
([`src/operator_cli/goal.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_cli/goal.rs)),
the goal is durably **tombstoned** (via `tombstone_goals`,
[`src/ooda_loop/curate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/curate.rs))
and dropped from the persisted goal board. Each OODA cycle the daemon reloads
the board and applies
[`filter_tombstoned`](https://github.com/rysweet/Simard/blob/main/src/goal_board_store/mod.rs),
so the goal never gets dispatched again.

But an engineer that was **already dispatched** for that goal — a
`simard engineer run single-process` subtree tracked in
`OodaState.engineer_worktrees[goal_id]`
([`src/ooda_loop/types.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/types.rs))
— **keeps running to completion**. Nothing compared the in-flight engineer set
against the freshly-loaded board, so a cancelled goal's engineer continued
producing PRs.

> **Live evidence.** After `simard goal remove
> simard-identity-concierge-hospitality-design-op-3719bfd4`, its engineer
> (tracked at `engineer_worktrees["simard-identity-concierge-hospitality-design-op-3719bfd4"]`)
> kept running **~70 minutes** and
> produced additional, unwanted PRs for a goal that no longer existed —
> wasting compute and creating merge noise.

The [stale-engineer-claim reaper](./stale-engineer-claim-reaper.md) does **not**
cover this case: that reaper reclaims *ledger claim slots* for engineers that
are already **provably dead** (no worktree / stale mtime). The engineer here is
very much **alive and productive** — it just has no goal anymore. Killing a
live-but-orphaned engineer is a distinct, additive reconciliation.

## What the reaper does

Each OODA cycle, right after the board is reloaded and tombstone-filtered, the
daemon calls `reap_engineers_for_tombstoned_goals`. For every entry in
`state.engineer_worktrees`, it asks a single durable question:

> **Is this engineer's `goal_id` in the tombstone set?**

If **yes**, the engineer is reaped in two independent, idempotent steps:

1. **Graceful termination.** The engineer's live `pid`/`session_name` are
   recovered by joining `goal_id` against the
   [subagent-session registry](../reference/subagent-tmux-tracking.md)
   ([`src/subagent_sessions/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/subagent_sessions/mod.rs)),
   selecting the **live** row (`ended_at` is `None`, most-recent `created_at`) —
   never a retained/ended row, whose recycled `pid` could belong to an unrelated
   process. A transient `SubordinateHandle` is built from that row and passed to
   the existing [`kill_subordinate`](https://github.com/rysweet/Simard/blob/main/src/agent_supervisor/lifecycle/mod.rs)
   primitive, which sends **SIGTERM** (never SIGKILL/`kill -9`) and tolerates
   `ESRCH` (already-exited) as success.
2. **Worktree cleanup.** The existing
   `cleanup_engineer_worktree_for_goal` chokepoint removes the
   `engineer_worktrees[goal_id]` entry, runs its guarded `.cleanup()` /
   `Drop` worktree removal, and releases the engineer claim through
   [`release_engineer_claim`](../reference/engineer-claim-release-api.md).

The two steps are **independent**: worktree cleanup runs **unconditionally**,
even when the registry lookup misses (no tmux, test env, or the process already
exited). SIGTERM is best-effort; cleanup is authoritative.

## Why tombstone membership is the only reap predicate

The reap decision keys on **exactly one signal**: `goal_id ∈ tombstones`. It
does **not** reap merely because a goal is absent from `board.active`.

| Goal state | Tombstoned? | Engineer reaped? |
|---|---|---|
| Active (still dispatchable) | no | **No** — healthy, keep working |
| Backlog / not yet promoted | no | **No** |
| Blocked (`[OODA-SAFEGUARD]` or dependency) | no | **No** — still on the board, may unblock |
| Paused / completion-pending finalization | no | **No** — transiently off `active`, still real |
| Removed via `simard goal remove` | **yes** | **Yes** — genuinely gone |
| Completed via `simard goal complete` | **yes** | **Yes** — genuinely gone |

The tombstone set is the **only durable, "genuinely GONE"** signal. It is
written by *both* `goal remove` and `goal complete`, it survives daemon
restarts, and nothing re-seeds a tombstoned id. Absence-from-`active` alone is
**not** sufficient — a goal can be transiently off the active list mid-curation,
during completion-pending finalization, or while Blocked, and reaping on that
signal would kill a producing engineer whose goal still exists. Keying on the
tombstone protects every Blocked / Paused / backlog goal **for free**.

## Binding guarantees

- **State-driven, never wall-clock.** A reap is triggered *only* by a durable
  tombstone, never by how long an engineer has been running. A busy engineer
  whose goal still exists is **never** reaped, no matter how long it runs. This
  is the project rule made mechanical: only reap engineers whose goal was
  cancelled.
- **Graceful, never `kill -9`.** Termination reuses `kill_subordinate`
  (SIGTERM + `ESRCH`-tolerant). There is **no** SIGKILL and **no** bespoke
  process-killer anywhere on this path.
- **Targets only the live process.** SIGTERM is aimed at the registry row with
  `ended_at == None` and the newest `created_at`, never a retained/ended row.
  Since `kill_subordinate` signals by `pid` alone and the registry keeps ended
  rows for up to 24h with potentially OS-recycled `pid`s, this live-row rule is
  what prevents SIGTERM from ever hitting an unrelated process.
- **Reuse, don't duplicate.** Termination goes through `kill_subordinate`;
  worktree removal and claim release go through
  `cleanup_engineer_worktree_for_goal` — the same chokepoints every other
  engineer-exit path uses. No parallel kill path, no hand-rolled `rm`, no
  hand-rolled SQL.
- **Idempotent and fail-safe.** Kill and cleanup are independent steps. A
  registry miss skips the (best-effort) SIGTERM but **always** runs cleanup.
  Every failure is logged and swallowed so reconciliation never aborts the
  OODA cycle.
- **Additive and self-healing.** A goal removed while its engineer was already
  in flight is reaped on the **next** OODA cycle. No migration, no operator
  action — the reconciliation catches it automatically.
- **No "Bridge".** No identifier, type, or module introduced by this feature is
  named "Bridge".

## Why the cycle reconciliation, not a direct signal from `goal remove`

`simard goal remove` / `simard goal complete` deliberately do **not** reach into
the daemon to kill the engineer directly. They only write the durable
tombstone. The daemon's per-cycle reconciliation is the single, robust
catch-all:

- It is **the** source of truth — one reap path, not two.
- It catches every way a goal can vanish (removed, completed, or gone on a
  board reload), including cases a direct CLI signal would miss.
- It avoids a parallel process-killer, honouring the "reuse existing
  mechanisms" constraint.

The cost is a reap latency of **at most one OODA cycle**, which is well within
the acceptance criteria ("reaped on the next cycle").

## Where it runs

The reconciliation is a **synchronous** step in the daemon's OODA loop
([`src/operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs)),
placed **inside** the board-reconciliation block, right after the
`filter_tombstoned` / `heal_stale_no_progress_blocks` assignment to
`state.active_goals` — the exact point where the persistent
`state.engineer_worktrees` and the block-scoped `cycle_tombstones` set are both
in scope, so the reap reuses the already-loaded tombstone set without a second
`load_tombstones`. There is **no new background thread and no independent
loop**; its cadence is the OODA cycle cadence. When it reaps ≥1 engineer it
emits one `daemon_log` line naming the count and the reaped `goal_id`s.

## Invariants at a glance

| Situation | Behaviour |
|---|---|
| In-flight engineer, goal tombstoned (removed) | **Reaped** — SIGTERM + worktree cleaned next cycle |
| In-flight engineer, goal tombstoned (completed) | **Reaped** — same path |
| In-flight engineer, goal still Active | **Kept** — never reaped |
| In-flight engineer, goal Blocked-but-present | **Kept** — Blocked is not tombstoned |
| In-flight engineer, goal transiently off `active` | **Kept** — absence-from-active is not a reap signal |
| Registry lookup misses (no tmux / test env) | SIGTERM skipped; **worktree cleanup still runs** |
| Registry has ended + live rows for the goal | SIGTERM targets the **live** row; ended/recycled `pid`s are ignored |
| Process already exited (`ESRCH`) | Treated as success; cleanup still runs |
| Reap step errors | Logged; cycle continues; other engineers still reconciled |

## Related

- [Stale-Engineer-Claim Reaper](./stale-engineer-claim-reaper.md) — the
  complementary sweep that reclaims *ledger claim slots* for engineers that are
  already **dead**; this reaper terminates engineers that are **alive but
  orphaned**.
- [Tombstoned-Goal Engineer Reaper API](../reference/tombstoned-goal-engineer-reaper-api.md)
  — the function signature, the registry join, the reuse seams, and the
  regression test list.
- [Goal board persistence](./goal-board-persistence.md) — how tombstones and the
  board are persisted and reloaded each cycle.
- [Subagent tmux tracking](../reference/subagent-tmux-tracking.md) — the
  registry that maps `goal_id → {session_name, pid}`.
- [Engineer-Claim Release & Reclaim API](../reference/engineer-claim-release-api.md)
- [Diagnose a reaped engineer after goal removal](../howto/diagnose-a-reaped-engineer-after-goal-removal.md)
