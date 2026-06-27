---
title: Concurrent engineer dispatch reference
description: How the Act phase dispatches spawn-path AdvanceGoal actions concurrently so multiple engineers start in a single OODA round, each with its own LLM session, bounded by the AIMD safety cap.
last_updated: 2026-06-26
owner: simard
doc_type: reference
status: reference
related:
  - ./maximum-safe-parallelism.md
  - ./goal-coverage-allocation.md
  - ./adaptive-scaling-api.md
  - ./goal-target-repo-routing.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
---

# Concurrent engineer dispatch reference

> **Goal:** In a single OODA round, dispatch **all** planned spawn-path
> `AdvanceGoal` actions (up to the AIMD `cap`) so their engineers start
> **concurrently** — not ~1 per round — without ever holding a global lock
> across the slow goal-action LLM call or the engineer spawn, and without
> double-spawning a goal.

Modules: `simard::ooda_actions` (`dispatch_actions_bounded`),
`simard::ooda_actions::concurrent` (`dispatch_advance_concurrent`).

## The problem (verified root cause)

Coverage ([goal coverage allocation](./goal-coverage-allocation.md)) already
plans up to `cap = scaler.current_max()` `AdvanceGoal` actions per cycle — one
per uncovered incomplete goal — and the #2405 fan-out turns a multi-issue
umbrella goal into several distinct goals so the plan is genuinely parallel
(see [maximum safe parallelism](./maximum-safe-parallelism.md)). The **plan**
was parallel; the **dispatch** serialized it.

`dispatch_actions` spawned one thread per `PlannedAction`, but every
non-`LaunchSession`/`SafeUpdate` action acquired a **global `bridges`+`state`
`Mutex` and held both for the entire `dispatch_one`**. For `AdvanceGoal` that
critical section included the slow goal-action LLM `run_turn` (~30–90 s) on the
**single shared `bridges.session`** plus the engineer spawn. So all
`AdvanceGoal` dispatches in a round serialized on that lock/session → only ~1
engineer effectively started per round even though coverage had planned many
(observed: "covered 7/7 incomplete goals (cap 8)" but engineers ramped ~1 per
cycle).

The engineer spawn itself is fast — it allocates a git worktree and launches a
**detached** background subprocess (`advance_goal::spawn`). The slow part held
under the lock was the LLM `run_turn`.

## Two-phase dispatch

`dispatch_actions_bounded(actions, bridges, state, max_concurrency)` partitions
the planned actions and dispatches them in two phases. `dispatch_actions` is a
thin wrapper that passes `max_concurrency = usize::MAX`; the Act phase calls the
bounded form with the AIMD cap.

| Phase | Actions | Concurrency | Behavior |
|-------|---------|-------------|----------|
| **Phase 1 — serialized** | `LaunchSession` and `SafeUpdate` (independent, no shared state); **assigned-goal heartbeat** `AdvanceGoal`; every non-`AdvanceGoal` kind | One thread per action; the non-independent kinds take a short global `bridges`+`state` lock | Unchanged from before — today's behavior over this subset. |
| **Phase 2 — concurrent** | **Unassigned spawn-path** `AdvanceGoal` (the engineer-spawn path) | Up to `max_concurrency` at once | New: each goal opens its own LLM session; the global lock is never held across `run_turn` or the spawn. |

Classification is `concurrent::is_concurrent_advance_candidate`: an
`AdvanceGoal` action whose goal exists and is currently **unassigned**
(`assigned_to.is_none()`) is a spawn candidate and goes to Phase 2. A goal with
a live subordinate routes to Phase 1 so its heartbeat-check behavior is
unchanged. Missing-goal / missing-id actions go to Phase 2, which surfaces the
appropriate failure outcome.

Outcomes are written back **by original index**, so `dispatch_actions_bounded`
returns one `ActionOutcome` per input action **in input order** — preserving
existing `ActionOutcome` semantics for downstream consumers.

## Per-thread LLM sessions

The reason serialized dispatch could not be made concurrent before is that all
threads shared one `bridges.session`. Phase 2 fixes this with a session
**factory**:

```rust
pub trait OrchestratorSessionFactory: Send + Sync {
    fn open_session(&self) -> SimardResult<Box<dyn BaseTypeSession>>;
}
```

- `OodaBridges.session_factory: Option<Arc<dyn OrchestratorSessionFactory>>`.
  The daemon wires `ProviderSessionFactory` (built via `SessionBuilder`), so
  each spawn-candidate goal mints its **own** session and the `run_turn` calls
  run in parallel.
- When `session_factory` is `None` (tests and non-daemon callers), Phase 2
  falls back to the single shared `bridges.session` **under a lock**, i.e.
  serialized — behavior is unchanged for those callers, and there is no silent
  loss of the LLM (it fails visibly if no session and no factory exist).

## Per-goal dispatch: lock discipline

Each Phase-2 thread runs `dispatch_advance_goal_concurrent`, structured so the
global `state` lock is held **only** for short critical sections and **never**
across the slow work:

1. **Acquire a semaphore permit** (the AIMD cap — see below).
2. **Phase 1 — short state lock.** Read the goal, run the status short-circuits
   (`Blocked` / `Completed` / `Proposed` / `Paused`) and the issue-#1911
   brain-failure auto-recovery, then **atomically claim** the goal
   (`try_claim`). If another concurrent thread already claimed it, return a
   benign "already claimed by a concurrent dispatch this round" outcome — **no
   double-spawn**. Re-snapshot the (possibly recovered) goal and the recalled
   `prepared_context`, then **release the lock**.
3. **Phase 2 — slow LLM call, no global lock.** Open a per-goal session from the
   factory (or take the shared-session fallback lock), build the turn input
   (`build_goal_advance_input`), call `run_turn`, and best-effort `close()` the
   session. The global `state` lock is **not** held here.
4. **Phase 3 — short state lock.** Apply the parsed decision
   (`apply_goal_advance_result`) under the lock, then — if the decision is
   `SpawnEngineer` — call `dispatch_spawn_engineer`. The spawn uses only short
   state critical sections (claim re-check, goal lookup, status/assignment
   writeback); **target-repo resolution, git worktree allocation, and the
   detached subprocess launch run with no lock held.**

`advance_goal_with_session` was split into `build_goal_advance_input` (lock
released) + `apply_goal_advance_result` (short lock) precisely so the slow
`run_turn` happens between them with no global lock.

## The AIMD safety cap (hard ceiling)

Concurrency is bounded by the [`AdaptiveScaler`](./adaptive-scaling-api.md), so
filling the machine stays resource-aware:

- The Act phase threads `scaler.current_max()` (the same value coverage used to
  bound the plan) from `cycle.rs` → `act()` → `dispatch_actions_bounded` as
  `max_concurrency`.
- A counting `Semaphore` (std has none; implemented with `Mutex` + `Condvar`)
  bounds the number of `AdvanceGoal` dispatches that hold a permit at once to
  `max_concurrency`. Concurrent engineer **starts** therefore never exceed the
  cap.
- The permit is released on `Drop` of its RAII guard, so a panicking dispatch
  thread still returns its permit and never wedges the round.
- Spawn errors still surface as failure outcomes and are reported to the scaler
  in `cycle.rs` (`report_reason` / `report_error`), so the additive-increase /
  multiplicative-decrease backoff under CPU/memory/429 pressure is fully
  preserved.

## Correctness and safety

- **No double-spawn (intra-round).** A per-round claim set (`HashSet<String>`
  behind a `Mutex`; `try_claim` is check-and-insert) guarantees each goal is
  claimed by exactly one thread.
- **No double-spawn (cross-round).** The existing `assigned_to` re-check inside
  `dispatch_spawn_engineer`, plus `find_live_engineer_for_goal`, remain as
  defense-in-depth against a goal that already has a live engineer from a prior
  round.
- **No data races.** All shared mutations of `OodaState` / the goal board stay
  behind short critical sections; the slow work (`run_turn`, worktree
  allocation, subprocess spawn) uses per-thread owned data.
- **Registry writes are lost-update safe.** Because engineers now spawn
  concurrently, `subagent_sessions::record_spawn` can run in parallel with
  itself and with the daemon's `poll_and_gc`. A process-wide registry mutation
  lock serializes each `load → mutate → save_atomic` cycle so no spawn entry is
  clobbered. (`save_atomic` keeps the file *valid* but not lost-update safe.)
- **Panic-safe / honest degradation.** Poisoned locks are recovered with
  `into_inner()` instead of panicking; a panicking dispatch thread yields a
  per-action failure outcome rather than aborting the round. **One failed
  action never aborts the others.** Failures surface explicitly — no silent
  degradation.

## Invariants

- **Cap is never exceeded.** At most `scaler.current_max()` engineer starts run
  concurrently per round; the scaler shrinks the cap under pressure.
- **Resource-aware.** The AIMD additive-increase / multiplicative-decrease
  behavior and 429/rate-limit backoff are unchanged.
- **At most one engineer per goal.** Intra-round atomic claim plus the
  cross-round `assigned_to` / live-worktree de-duplication.
- **Input order preserved.** One `ActionOutcome` per input action, returned in
  the same order.
- **Phase 1 behavior unchanged.** Heartbeat `AdvanceGoal`, `LaunchSession`,
  `SafeUpdate`, and every non-`AdvanceGoal` kind dispatch exactly as before.

## ⚠️ Deployment

This is a **Rust code change to the dispatch core** — it requires a **binary
rebuild + redeploy** and does **not** hot-reload. (Contrast the #2405 goal
**decomposition**, which is prompt-only and hot-reloads from
`~/.simard/prompt_assets/…`.) The operator deploys the new binary; do not
hot-reload the live daemon for this change.

## Related reading

- [Maximum safe parallelism](./maximum-safe-parallelism.md) — how coverage, the
  AIMD cap, and goal decomposition produce the parallel **plan** this dispatcher
  now realizes concurrently.
- [Goal coverage allocation](./goal-coverage-allocation.md) — the per-cycle
  allocator that plans one `AdvanceGoal` per uncovered incomplete goal, up to
  the cap.
- [Adaptive scaling API](./adaptive-scaling-api.md) — the AIMD scaler that
  supplies the resource-aware concurrency cap.
- [How OODA spawns engineer agents](../howto/spawn-engineers-from-ooda-daemon.md)
  — the prose goal-action contract and the spawn path Phase 2 feeds into.
- [Goal target-repo routing](./goal-target-repo-routing.md) — how each spawned
  engineer is routed to the correct repository.
