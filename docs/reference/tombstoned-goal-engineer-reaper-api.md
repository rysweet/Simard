---
title: "Reference: Tombstoned-Goal Engineer Reaper API"
description: >
  The API contract for the per-cycle reconciliation that reaps in-flight
  engineers whose goal was removed/completed: the
  reap_engineers_for_tombstoned_goals(state, tombstones, registry) function, its
  tombstone-only reap predicate, the subagent-session registry join that
  recovers pid/session_name, the reuse of kill_subordinate (SIGTERM) and
  cleanup_engineer_worktree_for_goal, the daemon call site, the idempotent /
  fail-safe error semantics, and the regression test list.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/tombstoned-goal-engineer-reaper.md
  - ./subagent-tmux-tracking.md
  - ./engineer-claim-release-api.md
  - ./engineer-worktree-isolation.md
  - ../reference/claim-reaper-api.md
---

# Reference: Tombstoned-Goal Engineer Reaper API

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary source:
> [`src/ooda_actions/advance_goal/subordinate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/subordinate.rs)
> (`reap_engineers_for_tombstoned_goals`).
> Conceptual overview:
> [Tombstoned-Goal Engineer Reaper](../concepts/tombstoned-goal-engineer-reaper.md).

## Overview

The reaper is a pure per-cycle reconciliation over three inputs that are all
already in scope in the daemon loop:

- the persistent **`OodaState`** (its `engineer_worktrees: HashMap<String,
  EngineerWorktree>` map, keyed by `goal_id`),
- the durable **tombstone set** (`HashSet<String>` of removed/completed goal
  ids), and
- the **subagent-session registry** (`goal_id → {session_name, pid}`).

It reuses two existing chokepoints — `kill_subordinate` and
`cleanup_engineer_worktree_for_goal` — and introduces **no new process-killer
and no new worktree-remover**.

## Function: `reap_engineers_for_tombstoned_goals`

The single public entry point, in
[`src/ooda_actions/advance_goal/subordinate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/subordinate.rs):

```rust
/// Reap every in-flight engineer whose goal has been tombstoned
/// (removed via `simard goal remove` or completed via `simard goal complete`).
///
/// Runs once per OODA cycle, right after the goal board is reloaded and
/// tombstone-filtered. State-driven and tombstone-gated: an engineer is reaped
/// iff its goal_id is in `tombstones`. Never a wall-clock timeout — a healthy
/// engineer whose goal is still on the board is never touched.
///
/// Two independent, idempotent steps per victim:
///   1. best-effort SIGTERM via a registry-recovered SubordinateHandle
///      (`kill_subordinate`, ESRCH-tolerant, never SIGKILL);
///   2. always `cleanup_engineer_worktree_for_goal` (removes the map entry,
///      runs the guarded worktree `.cleanup()`/Drop, releases the claim).
///
/// Per-victim errors are contained and logged; the reconciliation never aborts
/// the OODA cycle. Returns the `goal_id`s reaped this cycle (empty if none), so
/// the caller can log exactly which orphaned engineers were terminated.
pub fn reap_engineers_for_tombstoned_goals(
    state: &mut OodaState,
    tombstones: &HashSet<String>,
    registry: &subagent_sessions::Registry,
) -> Vec<String>;
```

### Reap predicate (tombstone-only)

```text
victims = { goal_id in state.engineer_worktrees.keys()
            : goal_id in tombstones }
```

- **Absence from `board.active` is NOT a predicate.** Only tombstone membership
  triggers a reap. This is the whole safety story: Blocked, Paused, backlog, and
  completion-pending goals are never tombstoned, so their engineers are never
  reaped.
- Victim `goal_id`s are **collected first** (into an owned `Vec<String>`) before
  any mutation, so the subsequent `&mut state` reaping never overlaps a borrow
  of `state.engineer_worktrees`.

### Algorithm

```text
victims = collect goal_ids in state.engineer_worktrees where goal_id ∈ tombstones
for goal_id in victims:
    // Step 1 — best-effort graceful termination (skipped on registry miss)
    //   Select the LIVE row, not any historical retry: filter to
    //   ended_at.is_none() and pick the most-recent created_at. The registry
    //   retains ended rows for up to RETENTION_SECONDS (24h) and may hold
    //   several rows per goal_id, so a naive `.find()` could target a stale
    //   pid that the OS has since recycled.
    if let Some(session) = registry.sessions.iter()
            .filter(|s| s.goal_id == goal_id && s.ended_at.is_none())
            .max_by_key(|s| s.created_at):
        let mut handle = SubordinateHandle::from_session(session)  // live pid, agent_name, goal, session_name
        match kill_subordinate(&mut handle):
            Ok(())                          => log SIGTERM sent
            Err(_) /* incl. ESRCH-mapped */ => log and continue   // never fatal
    // Step 2 — authoritative cleanup (ALWAYS runs)
    cleanup_engineer_worktree_for_goal(state, &goal_id)   // map remove + worktree Drop + claim release
if !victims.is_empty(): daemon_log("[simard] reaped N engineer(s) for tombstoned goal(s): id1, id2, …")
return victims   // the reaped goal_ids
```

- **No `.unwrap()`/`.expect()`** on any I/O, signal, or filesystem path.
- **Per-victim containment:** an error terminating or cleaning one engineer is
  logged and the loop continues to the next victim.
- **Chokepoints only:** SIGTERM via `kill_subordinate`; worktree removal + claim
  release via `cleanup_engineer_worktree_for_goal`. No bespoke `rm`, no
  hand-rolled SQL, no `--admin`.

## Termination seam: `kill_subordinate` (reused)

Reused unchanged from
[`src/agent_supervisor/lifecycle/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/agent_supervisor/lifecycle/mod.rs):

```rust
pub fn kill_subordinate(handle: &mut SubordinateHandle) -> SimardResult<()>;
```

- On Unix it sends **`SIGTERM`** via `libc::kill(pid, SIGTERM)`.
- **`ESRCH`** (process already exited) is mapped to success — the engineer may
  have finished between board reload and reconciliation.
- `pid == 0` sends **no** signal (safe in test envs) — cleanup still runs.
- The handle is marked `killed = true`; there is **no SIGKILL escalation**.

Because `engineer_worktrees` stores `EngineerWorktree` (not a live
`SubordinateHandle`), the reaper reconstructs a transient `SubordinateHandle`
from the matching **live** registry row (`pid`, `agent_name`/`agent_id`, `goal`,
`session_name`), then passes it to `kill_subordinate`. `SubordinateHandle` has
no `Default` and eight required fields, so the reconstruction populates all of
them explicitly — the targeting-relevant `pid` and `session_name` from the
registry row, `killed: false`, and inert placeholders for the fields
`kill_subordinate` never reads (`worktree_path`, `spawn_time`, `retry_count`):

```rust
// illustrative — all eight fields are set explicitly (no `..` shorthand)
let mut handle = SubordinateHandle {
    pid: session.pid,
    agent_name: session.agent_id.clone(),
    goal: goal_id.clone(),
    worktree_path: worktree.path.clone(),
    spawn_time: session.created_at as u64,
    retry_count: 0,
    killed: false,
    session_name: session.session_name.clone(),
};
```

This is a *targeting* handle only — it signals the exact `pid` the registry
recorded for the **live** row of that `goal_id`, never a name scan or a
process-tree sweep.

## Cleanup seam: `cleanup_engineer_worktree_for_goal` (reused)

Reused unchanged, in the same module
([`src/ooda_actions/advance_goal/subordinate.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/subordinate.rs)):

```rust
fn cleanup_engineer_worktree_for_goal(state: &mut OodaState, goal_id: &str);
```

- Removes `state.engineer_worktrees[goal_id]` and runs the guarded
  `EngineerWorktree::cleanup()` (Drop is the safety net if `cleanup()` was
  skipped).
- Calls `release_engineer_claim_for_goal` →
  [`release_engineer_claim`](./engineer-claim-release-api.md) — the idempotent,
  fail-visible ledger `DELETE` chokepoint.
- **Idempotent:** a missing map entry is a silent no-op, so calling it for an
  already-exited engineer is safe.

This is exactly the cleanup every terminal engineer-exit path already uses; the
reaper adds **no** second removal path.

## Registry join: `subagent_sessions::Registry`

The live `pid`/`session_name` come from the subagent-session registry
([`src/subagent_sessions/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/subagent_sessions/mod.rs)),
loaded once per cycle with `subagent_sessions::load()`:

```rust
pub struct SubagentSession {
    pub agent_id: String,
    pub session_name: String,
    pub host: String,
    pub pid: u32,
    pub created_at: i64,
    pub ended_at: Option<i64>,   // None ⇒ still live
    pub goal_id: String,         // ← join key
}

pub struct Registry { pub sessions: Vec<SubagentSession> }
```

**Selecting the live row is mandatory, not cosmetic.** The registry keeps ended
rows for up to `RETENTION_SECONDS` (24h) and records a fresh row per retry, so
a single `goal_id` can map to several rows — most of them ended, with `pid`s the
OS may have recycled onto unrelated live processes. Because `kill_subordinate`
signals **by `pid` only** (it never re-checks `session_name`), targeting a stale
row could SIGTERM an innocent process. The reaper therefore filters to
`ended_at.is_none()` and takes `max_by_key(created_at)`:

```rust
registry.sessions.iter()
    .filter(|s| s.goal_id == goal_id && s.ended_at.is_none())
    .max_by_key(|s| s.created_at)
```

A miss (registry unavailable, all rows GC'd/ended, or a test env with no tmux)
**skips only the SIGTERM** — cleanup still runs, because cleanup is
authoritative and the process, if any, exits on its own or is caught by the
[stale-claim reaper](./claim-reaper-api.md).

## Daemon call site

Invoked from the OODA loop in
[`src/operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs),
**inside** the per-cycle board-reconciliation block — the same block that
computes `cycle_tombstones` and assigns `state.active_goals`. `cycle_tombstones`
is block-scoped, so the reap call must live **within** that block (right after
the `state.active_goals = …` assignment) to reuse the already-loaded tombstone
set without a second `load_tombstones`:

```rust
// … inside the reconciliation block, where `cycle_tombstones` is in scope …
state.active_goals =
    heal_stale_no_progress_blocks(filter_tombstoned(board, &cycle_tombstones));

let registry = crate::subagent_sessions::load();
let reaped_goal_ids = crate::ooda_actions::advance_goal::subordinate::reap_engineers_for_tombstoned_goals(
    &mut state,
    &cycle_tombstones,
    &registry,
);
if !reaped_goal_ids.is_empty() {
    daemon_log(
        &state_root,
        &format!(
            "[simard] OODA cycle: reaped {} in-flight engineer(s) for tombstoned goal(s): {}",
            reaped_goal_ids.len(),
            reaped_goal_ids.join(", "),
        ),
    );
}
```

The emitted log line names the reaped `goal_id`s (not just the count) so an
operator can confirm exactly which orphaned engineers were terminated. There is
**no new thread**; the call is synchronous within the existing cycle.

## Error semantics (fail-safe, fail-visible)

| Path | On error |
|---|---|
| No **live** registry row for `goal_id` (only ended rows, or none) | SIGTERM skipped; **cleanup still runs**; logged at debug |
| Registry `goal_id` lookup misses (registry unavailable / test env) | SIGTERM skipped; **cleanup still runs**; logged at debug |
| `kill_subordinate` SIGTERM fails (non-`ESRCH`) | logged; cleanup still runs; loop continues |
| `kill_subordinate` returns `ESRCH` | treated as success (already exited); cleanup still runs |
| `EngineerWorktree::cleanup()` fails | logged; `Drop` runs as the safety net; loop continues |
| `release_engineer_claim` `DELETE` fails | logged; loop continues (stale-claim reaper is the backstop) |

Nothing on the reap path swallows a failure silently, and no single victim's
failure aborts the reconciliation.

> **PID-reuse safety.** Because ended registry rows are retained for up to 24h
> and their recorded `pid`s can be recycled by the OS onto unrelated live
> processes, the reaper only ever targets a row with `ended_at.is_none()`
> (most-recent `created_at`). This guarantees SIGTERM is aimed at the engineer
> actually spawned for the tombstoned goal, never a stale pid.

## Regression coverage

Tests live inline in `src/ooda_actions/advance_goal/subordinate.rs`
(`#[cfg(test)]`), using a temp `SIMARD_STATE_ROOT` and `pid: 0` handles so no
real process is signalled:

| Test | Asserts |
|---|---|
| (a) Removed goal reaps its engineer | An in-flight engineer whose `goal_id` **is tombstoned** is reaped on the next cycle: the `engineer_worktrees` entry is gone **and** its worktree directory is cleaned. |
| (b) Healthy engineer is preserved | An engineer whose goal is **still present / not tombstoned** is **not** reaped: the `engineer_worktrees` entry and worktree survive. |
| (c) Blocked-but-present goal is preserved | An engineer whose goal is on the board in **Blocked** state (not tombstoned) is **not** reaped. |
| (d) Live-row selection ignores ended rows | With both an **ended** row (old `pid`) and a **live** row (`ended_at: None`, newer `created_at`) for the same tombstoned `goal_id`, the handle is built from the **live** row's `pid` — the stale/recycled pid is never targeted. |
| Idempotency / registry-miss | With **no** matching registry row, `reap_…` still removes the map entry and cleans the worktree (SIGTERM skipped, cleanup authoritative); a second call is a no-op. |
| `pid == 0` safety | Reaping with a `pid: 0` handle sends no signal and does not error. |
| Return value | `reap_…` returns exactly the tombstoned `goal_id`s it reaped (empty when none), matching the daemon's log line. |

Required gates (merge blockers), matching
[`.github/workflows/verify.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/verify.yml):
`cargo fmt --all -- --check`,
`cargo clippy --all-targets --all-features --locked -- -D warnings`, and
`cargo test --all-features --locked --no-fail-fast` must pass; the
reap/kill/cleanup paths carry no `unwrap`/`expect` on I/O.

## Related

- [Tombstoned-Goal Engineer Reaper (concept)](../concepts/tombstoned-goal-engineer-reaper.md)
- [Subagent tmux tracking](./subagent-tmux-tracking.md) — the `goal_id →
  {session_name, pid}` registry the reaper joins against.
- [Engineer-Claim Release & Reclaim API](./engineer-claim-release-api.md)
- [Engineer worktree isolation](./engineer-worktree-isolation.md)
- [Stale-Engineer-Claim Reaper API](./claim-reaper-api.md) — the complementary
  sweep for engineers that are already dead.
