---
title: Brain-relative OODA cycle counter
description: Why Simard's OODA "Cycle #N" counts the brain's total lived cognition — a durable, monotonic number that continues across daemon restarts and deploys — instead of the current process's uptime, which reset the dashboard to "Cycle #1" on every deploy and erased the visible sense of accumulated thought.
last_updated: 2026-07-06
owner: simard
doc_type: concept
related:
  - ./authoritative-goal-board-store.md
  - ./perpetual-goal-no-progress-exemption.md
  - ../reference/durable-ooda-cycle-counter.md
  - ../reference/dashboard-activity-cycle-reports.md
  - ../reference/dashboard-thinking-cycle-history.md
  - ../howto/inspect-the-ooda-cycle-counter.md
---

# Brain-relative OODA cycle counter

The OODA "Cycle #N" now reflects **the brain's total lived cognition** — a
durable, monotonically increasing number that continues across every daemon
restart and deploy — rather than the **current process's uptime**, which reset
to `1` every time the daemon started.

The operator's question framed it exactly:

> *"Should not the cycle counts be relative to the brain memory instead of the
> daemon runtime?"*

Yes. This page explains why, and what "brain-relative" means concretely.

## The problem: the counter was process-relative

The OODA cycle number lived only in process memory
([`OodaState::cycle_count`](../reference/durable-ooda-cycle-counter.md#durable-field-persistentgoalstatecycle_count)),
initialised to `0` in `OodaState::new()` and incremented once per cycle.
Alongside it, a second process-local `cycles_run` counter (which drives the
`--cycles` stop condition) also started at `0` on every launch.

Simard is deployed and restarted frequently — every self-deploy, every crash
recovery, every config change execs a fresh daemon. Each restart reset both
counters to `0`, so:

- the very next `cycle=` log line read `cycle=1`;
- the next persisted `cycle_<N>.json` report was written as `cycle_1.json`;
- the `daemon_health.json` heartbeat wrote `cycle_number: 1`;
- and the dashboard's "Cycle #N" — fed from that health field — showed
  **"Cycle #1"** again.

The dashboard displayed **"Cycle #1" over and over**, deploy after deploy. To an
operator watching, it looked like Simard kept starting from scratch and *nothing
was accumulating* — the exact opposite of the truth, where the brain had already
lived through hundreds or thousands of cycles. The sense of accumulated cognitive
activity was erased on a cadence measured in hours.

This is the same class of defect the dashboard reconciliation in
[Activity — Cycle Reports](../reference/dashboard-activity-cycle-reports.md)
patched at the *display* layer (issue #1680): a health-driven `#1` disagreeing
with the report-driven cumulative count. That fix made the panels agree by taking
the `max()` of the two. This change fixes it at the **source**: there is now one
durable number, so the two inputs to that `max()` no longer contradict.

## The principle: count lived cognition, not uptime

A cycle number should answer *"how much has this mind thought, in total?"* — a
property of the **brain**, which is durable — not *"how long has this process been
up?"*, a property of the **runtime**, which is ephemeral.

Simard already treats the brain as durable in exactly this way for a sibling
counter. The [no-progress breaker](../reference/no-progress-breaker-api.md)'s
per-goal `NoProgressTracker` had the identical bug — it reset to zero on each
restart and so could never reach its threshold — and it was fixed by persisting
it in the [authoritative goal-board store](./authoritative-goal-board-store.md)
(`<state_root>/state/goal_board.json`) so it survives restarts. The cycle counter
is the same story: it belongs to the brain's persistent memory, so it lives in
the same durable store.

Concretely, "brain-relative" means:

1. **Persisted.** The counter is a field of the durable, `flock`-guarded,
   atomically-rewritten `PersistentGoalState` — the brain's cognitive-memory
   state, not process memory.
2. **Continued, not reset.** On startup the daemon **loads** the last cycle
   number and continues the monotonic sequence (`next = last + 1`) instead of
   starting at `0`/`1`.
3. **Monotonic.** A `max()` guard on write means the count can only ever advance;
   no stale value, concurrent writer, or restart race can rewind it.
4. **Bounded loss.** It is re-persisted on every cycle and on shutdown, so a
   crash loses at most the one in-flight cycle.
5. **One number everywhere.** The `cycle=` logs, the cycle reports, the journal,
   the telemetry, and every dashboard "Cycle #N" all project from this single
   durable value.

A genuinely fresh brain — an empty state root with no prior cognition — still
starts at `#1` and increments from there. Continuity is a property of a brain's
*memory*, so a new memory legitimately begins its own count.

## Why persist in the goal-board store (not a new home)

The counter is durable brain state, and Simard already has exactly one durable
brain-state file with the right properties: `goal_board.json`, written under a
cross-process `flock` with an atomic temp-file-plus-`rename` read-modify-write.
It already persists the goal board and the no-progress tracker for this same
"must survive restart" reason.

Adding one `#[serde(default)] u32` field there:

- reuses the existing atomic, locked transaction — no new lock, file, or write
  window, and no new failure mode;
- inherits the store's fail-soft load (a corrupt or older file degrades the field
  to `0`, never failing the load);
- keeps the schema version at `1` because the field is additive and
  serde-default-compatible;
- follows a proven precedent (the no-progress tracker fix) rather than inventing a
  parallel persistence path.

This satisfies the memory-architecture guideline: the counter *is* part of the
brain's persistent memory, and it is stored there — not left process-local, and
not forked into a separate mechanism when the natural durable OODA-state store
already exists.

## What the operator sees now

- After a deploy, the dashboard "Cycle #N" **continues** from where it was — e.g.
  a restart during cycle 1,204 shows the next cycle as `#1,205`, never `#1`.
- The `cycle=` field in `journalctl` and the persisted `cycle_<N>.json` reports
  keep climbing across restarts.
- The Activity "Cycle Reports" card, the Thinking tab's Cycle History, and the
  System Status counter all show the same durable, brain-relative number.
- A brand-new brain (fresh state root) still starts at `#1`.

The number once again communicates what it should: the accumulated thought of a
long-lived mind, uninterrupted by the churn of the process hosting it.

## See also

- [Durable OODA cycle counter API reference](../reference/durable-ooda-cycle-counter.md) — the field, the `commit_cycle` `max()` guard, the startup seed, the backfill, and the health-write repointing.
- [Authoritative goal-board store](./authoritative-goal-board-store.md) — the durable store the counter rides on.
- [No-progress breaker API](../reference/no-progress-breaker-api.md) — the sibling counter persisted in the same store for the same reason.
- [Activity tab — Cycle Reports](../reference/dashboard-activity-cycle-reports.md) — the #1680 display-layer reconciliation this change resolves at the source.
- [How to inspect the OODA cycle counter](../howto/inspect-the-ooda-cycle-counter.md) — verify continuity across a restart.
