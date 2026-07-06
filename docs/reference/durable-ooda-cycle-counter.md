---
title: Durable OODA cycle counter API reference
description: Reference for the brain-relative, durable OODA cycle counter — the persisted `PersistentGoalState.cycle_count` field in the authoritative goal-board store, the monotonic `max()` guard in `commit_cycle`, the daemon startup seed, the one-time report backfill, the repointed `daemon_health.json` writes, and the single source of truth that keeps every `cycle=` log line, cycle report, and dashboard "Cycle #N" counting the brain's total lived cognition instead of resetting to 1 on each daemon restart.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/brain-relative-ooda-cycle-counter.md
  - ../concepts/authoritative-goal-board-store.md
  - ../reference/no-progress-breaker-api.md
  - ../reference/goal-board-api.md
  - ../reference/dashboard-activity-cycle-reports.md
  - ../reference/dashboard-thinking-cycle-history.md
  - ../howto/inspect-the-ooda-cycle-counter.md
  - ../../src/goal_board_store/mod.rs
  - ../../src/ooda_loop/types.rs
  - ../../src/ooda_loop/cycle.rs
  - ../../src/operator_commands_ooda/daemon/mod.rs
  - ../../src/operator_commands_dashboard/cycle_source.rs
---

# Durable OODA cycle counter API reference

> **Status: implemented.** The durable counter is the
> `cycle_count` field on
> [`PersistentGoalState`](https://github.com/rysweet/Simard/blob/main/src/goal_board_store/mod.rs),
> serialised into the authoritative `<state_root>/state/goal_board.json` store.
> The daemon seeds it at startup, advances the in-memory
> [`OodaState::cycle_count`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/types.rs)
> each cycle, and persists it under the store's `flock` on every `commit_cycle`.
> The dashboard renders it through
> [`cycle_source::authoritative_cycle_number`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_dashboard/cycle_source.rs).

This reference specifies the API of the **brain-relative** OODA cycle counter:
the number that reflects Simard's *total lived cognition* across the brain's
whole life, monotonically increasing across every daemon restart and deploy —
not the current process's uptime. For the rationale and the "should the cycle
count be relative to the brain memory instead of the daemon runtime?" narrative,
see [Brain-relative OODA cycle counter](../concepts/brain-relative-ooda-cycle-counter.md).

## Contents

- [The two counters](#the-two-counters)
- [Durable field: `PersistentGoalState.cycle_count`](#durable-field-persistentgoalstatecycle_count)
- [Monotonic persistence: `commit_cycle`](#monotonic-persistence-commit_cycle)
- [Startup seed](#startup-seed)
- [One-time report backfill](#one-time-report-backfill)
- [Daemon health writes](#daemon-health-writes)
- [Single source of truth for display](#single-source-of-truth-for-display)
- [Auto-corrected derivations](#auto-corrected-derivations)
- [Guarantees and non-guarantees](#guarantees-and-non-guarantees)
- [Tests](#tests)
- [What is unchanged](#what-is-unchanged)
- [See also](#see-also)

## The two counters

Two distinct counters exist. Only the first is displayed.

| Counter | Type | Scope | Persisted | Drives |
| --- | --- | --- | --- | --- |
| **`cycle_count`** (canonical) | `u32` | The **brain's** total lived cognition. Monotonic across restarts/deploys. | **Yes** — `PersistentGoalState.cycle_count` | Every `cycle=` log line, `CycleReport.cycle_number`, `cycle_<N>.json` filename, `daemon_health.json.cycle_number`, and the dashboard "Cycle #N". |
| `cycles_run` (secondary) | `u32` | **This process / this session** uptime. Resets to `0` on every daemon start. | No | The `--cycles` (`max_cycles`) stop condition **only**. May be surfaced as an optional "this session" metric. |

Before this feature both the in-memory `OodaState::cycle_count` **and**
`cycles_run` reset to `0` on every process start, so the displayed number reset
to `#1` on each deploy. `cycle_count` is now durable; `cycles_run` is demoted to
the session-scoped stop condition and no longer feeds any displayed number.

## Durable field: `PersistentGoalState.cycle_count`

The counter lives on the existing durable, `flock`-guarded, atomically-rewritten
[`PersistentGoalState`](../concepts/authoritative-goal-board-store.md) — the same
store that already persists the goal board and the
[`NoProgressTracker`](./no-progress-breaker-api.md) so those survive the daemon's
periodic restarts. It is exactly the same durability precedent, applied to one
more counter.

```rust
/// The complete durable goal-board state, serialised to `goal_board.json`.
///
/// Every field carries `#[serde(default)]` so a partially-written or older
/// file still deserialises into a usable value rather than failing the load.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistentGoalState {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub board: GoalBoard,
    #[serde(default)]
    pub no_progress: NoProgressTracker,
    /// Monotonic OODA cycle counter reflecting the BRAIN's total lived
    /// cognition. Seeded into `OodaState::cycle_count` at daemon startup and
    /// re-persisted (under the store `flock`) on every `commit_cycle`, so the
    /// number continues across daemon restarts and deploys instead of resetting
    /// to 1. A fresh brain (no prior file) defaults to `0`; the first cycle's
    /// `+= 1` makes the first displayed cycle `#1`.
    #[serde(default)]
    pub cycle_count: u32,
}
```

The field is **additive** and `#[serde(default)]`, so:

- `STORE_VERSION` stays `1`. No migration code is required.
- A legacy `goal_board.json` written before the field existed deserialises with
  `cycle_count == 0` and is self-healing: the next `commit_cycle` re-stamps the
  field (see [backfill](#one-time-report-backfill) for how a busy legacy brain
  avoids a visible dip to `#0`/`#1`).
- A corrupt or partially-written file degrades to `0` rather than failing the
  load, matching the store's existing fail-soft deserialization contract.

## Monotonic persistence: `commit_cycle`

`commit_cycle` gains a `cycle_count` parameter and stamps it inside the store's
existing atomic read-modify-write closure (the same `flock` + temp-file +
`rename` transaction that already persists the reconciled board and the
tracker). There is **no** new lock, no new file, and no additional write window.

```rust
/// Commit the daemon's post-cycle board authoritatively.
///
/// Steps, all durable and under the store lock:
/// 1. Record `new_tombstones` so no path can recreate archived/completed goals.
/// 2. Re-read the current file, `reconcile` the in-flight board against it
///    honouring tombstones, and persist the reconciled board, the `tracker`,
///    and the monotonic `cycle_count`.
pub fn commit_cycle(
    state_root: &Path,
    in_flight: &GoalBoard,
    tracker: &NoProgressTracker,
    cycle_count: u32,
    new_tombstones: &[String],
) -> SimardResult<GoalBoard> {
    // ...
    mutate(state_root, move |s| {
        let reconciled = reconcile(&s.board, &in_flight, &tombstones);
        s.board = reconciled.clone();
        s.no_progress = tracker;
        // Monotonic guard: the counter can only ever advance. A stale in-memory
        // value, a concurrent writer, or a hand-edited lowered file can never
        // rewind the brain's lived-cognition count.
        s.cycle_count = s.cycle_count.max(cycle_count);
        reconciled
    })
}
```

The `max()` is the integrity guard: `cycle_count` is **write-once-per-cycle,
never-decreasing**. The parameter is **required** (no default), so the compiler
flags every caller — there is no silent path that forgets to persist it.

Both daemon call sites pass the post-increment `state.cycle_count`:

| Call site | File | When | Value passed |
| --- | --- | --- | --- |
| Per-cycle commit | `daemon/mod.rs` | end of every cycle, after `run_ooda_cycle` incremented the counter | `state.cycle_count` |
| Shutdown commit | `daemon/mod.rs` | best-effort on graceful shutdown | `state.cycle_count` |

## Startup seed

Durable load happens at the daemon startup site, right where the persisted
`NoProgressTracker` is already restored. `OodaState::new()` itself is
**unchanged** and still initialises `cycle_count: 0` — the seed is applied by the
daemon after construction, so the `new()` invariant (`cycle_count == 0`) that
existing unit tests assert is preserved.

```rust
let persistent =
    crate::goal_board_store::load_or_migrate(&state_root, &*bridges.memory).unwrap_or_default();
// ...
let mut state = OodaState::new(board);
state.no_progress_tracker = persistent.no_progress;
// Issue #1 (brain-relative cycle counter): continue the brain's monotonic OODA
// cycle sequence across this restart instead of resetting to 0/1. The seed is
// `persistent.cycle_count`, unless the one-time report backfill (see below)
// raises it for a brain upgraded from a build that never persisted the field.
// The first `run_ooda_cycle` then advances to `seed + 1`.
state.cycle_count = seed;
```

The one-time [report backfill](#one-time-report-backfill) is applied **at this
same startup site**, immediately before the assignment above — it may raise the
seed above `persistent.cycle_count`, but never lowers it.

The **per-cycle re-sync** (which reloads the authoritative board and restores the
tracker at the top of every cycle) does **not** rewind `cycle_count`: the
in-memory value is always `>=` the persisted value mid-cycle, and the store's
`max()` guard makes any reload race harmless. Only the startup site seeds the
counter.

## One-time report backfill

A brain upgraded from a build that never persisted `cycle_count` starts with the
field defaulting to `0`, even though thousands of `cycle_<N>.json` reports on
disk prove a high cumulative count. To avoid a one-deploy visible dip to `#1`,
the **daemon startup site** (in `daemon/mod.rs`, right after `load_or_migrate`
returns and before the counter is seeded into `OodaState` — see
[Startup seed](#startup-seed)) performs a **guarded, idempotent, read-only**
backfill:

```rust
// At the daemon startup site, after `load_or_migrate` returns `persistent`.
let mut seed = persistent.cycle_count;
if seed == 0 {
    // Recover the count from the highest cycle-report filename index so the
    // display does not dip to #1 for a single deploy after the upgrade.
    let latest = cycle_source::latest_persisted_cycle_number(&state_root); // u64
    if latest > 0 {
        seed = u32::try_from(latest).unwrap_or(u32::MAX); // saturating u64 -> u32
    }
}
state.cycle_count = seed;
```

Properties:

- **Guarded by `== 0`.** Once the field is non-zero it is authoritative and the
  backfill never runs again — a single successful `commit_cycle` makes the guard
  permanently false.
- **At the daemon startup site, not inside a shared load primitive.** The
  backfill lives in the daemon's startup path in `daemon/mod.rs`, **not** inside
  `load_or_migrate` or `load()`. Both are shared read primitives:
  `load_or_migrate` is also the `simard goal` CLI read/write path
  (`operator_cli/goal.rs`), and when the store file already exists it simply
  delegates to `load()`. Keeping the backfill out of them means the generic
  `load()` retains its "returns exactly the last committed state" contract, and
  CLI/dashboard reads pay no directory scan and never observe a synthesized
  value — only the daemon, once, at startup, does.
- **Read-only over local filenames, no layering inversion.** It reuses
  `cycle_source::latest_persisted_cycle_number`, which only inspects
  `cycle_<N>.json` filenames in the two report directories — it never reads
  report bodies, touches the network, or writes at load time. Running the
  backfill in the daemon module (rather than embedding it in the low-level
  `goal_board_store`) is what keeps the dependency arrow pointing the right way:
  the low-level store never has to depend on the dashboard's `cycle_source`.
  Optionally, `latest_persisted_cycle_number` could be re-homed to a low-level
  cycle-report module that both the daemon and the dashboard read from, but that
  is a tidiness choice, not a requirement for this placement.
- **`u64` -> `u32` narrowing.** `latest_persisted_cycle_number` returns `u64`
  while `cycle_count` is `u32`; the seed narrows with a saturating conversion
  (`u32::try_from(latest).unwrap_or(u32::MAX)`).
- **Cannot rewind.** Paired with the `commit_cycle` `max()` guard, the backfill
  can only ever raise the counter to match on-disk reality.
- **Fresh brain stays at 0.** With no reports, `latest_persisted_cycle_number`
  returns `0`, the guard's second clause is false, and the counter stays `0` so
  the first cycle is `#1`.

## Daemon health writes

The `daemon_health.json` heartbeat's `cycle_number` (and the `actions_taken`
"Starting cycle #N" string) are repointed from the process-local `cycles_run + 1`
to the durable counter. Ordering matters and is verified:

| Write | File | Fires | Value |
| --- | --- | --- | --- |
| Pre-cycle heartbeat (`status: "running"`) | `daemon/mod.rs` | **before** `run_ooda_cycle` increments the counter | `state.cycle_count + 1` (the cycle about to run) |
| Post-cycle heartbeat (`status: "healthy"`, `cycle_phase: "sleep"`) | `daemon/mod.rs` | **after** `run_ooda_cycle` and `commit_cycle` | `state.cycle_count` (the cycle just finished) |

Both name the **same** cycle number for a given iteration, and both are the
durable brain-relative value. `cycles_run` no longer appears in any health field.

## Single source of truth for display

Every dashboard panel already funnels its "Cycle #N" through
[`cycle_source::authoritative_cycle_number`](./dashboard-activity-cycle-reports.md#relationship-to-the-authoritative-cycle-counter):

```rust
pub(crate) fn authoritative_cycle_number(state_root: &Path, daemon_health: Option<&Value>) -> u64 {
    health_cycle_number(daemon_health).max(latest_persisted_cycle_number(state_root))
}
```

Once `daemon_health.json.cycle_number` and the `cycle_<N>.json` filenames are
both the durable monotonic value, the two inputs to this `max()` **agree**, so
the dashboard shows the brain-relative number on Overview, Whiteboard, System
Status, the Activity "Cycle Reports" card (#26), and the Thinking tab's Cycle
History (#21) — no more disagreement between a health-driven `#1` and a
report-driven `#1159` (#1680). The `max()` is retained as a defensive safety net;
it is now the reconciliation of two sources that already match rather than a
patch over a contradiction.

## Auto-corrected derivations

Because every surface *projects* from the single durable `cycle_count`, the
following require **no code change** — they become brain-relative automatically
once the counter is durable:

| Surface | Source | File |
| --- | --- | --- |
| `cycle=` tracing span field | `fields(cycle = state.cycle_count)` | `ooda_loop/cycle.rs` |
| `CycleReport.cycle_number` | `cycle_number: state.cycle_count` | `ooda_loop/cycle.rs` |
| Persisted `cycle_<N>.json` filename index | derived from `cycle_number` | `ooda_loop` cycle-report persistence |
| Activity "Cycle Reports" card (#26) | shared `cycle_source` reader | `operator_commands_dashboard/cycle_source.rs` |
| Thinking tab Cycle History (#21) | shared `cycle_source` reader | `operator_commands_dashboard/cycle_source.rs` |
| System Status / Overview / Whiteboard counter | `authoritative_cycle_number` | `operator_commands_dashboard/*` |

> **Note — `cycle=` off-by-one.** The `cycle=` tracing span field is evaluated by
> `#[tracing::instrument(skip_all, fields(cycle = state.cycle_count))]` at
> `run_ooda_cycle` **entry**, before the per-cycle `state.cycle_count += 1`
> executes later in the body. So the `cycle=` log line trails
> `CycleReport.cycle_number`, the health `cycle_number`, and the dashboard
> "Cycle #N" by one for the same cycle. This is pre-existing behaviour that the
> durable counter does **not** change (cycle.rs is untouched); it is called out
> so the log value is not misread as a regression.

The recipe OODA-step helper (`bin/simard_ooda_step.rs`) is **out of scope** for
this feature and does **not** read `goal_board.json`. It carries the cycle number
through the `OodaStateSnapshot` JSON the recipe runner round-trips between phases,
independent of the durable store. Durability of that path is a property of the
recipe runner's state hand-off, not of `PersistentGoalState`.

## Guarantees and non-guarantees

**Guaranteed:**

- **Monotonic across restarts.** After any clean or crashing restart the counter
  continues from the last committed value; the first post-restart cycle is
  `last + 1`, never `1`.
- **Bounded loss on crash.** The counter is persisted on **every** `commit_cycle`
  (end of every cycle) and on graceful shutdown, so an unclean crash loses at
  most the single in-flight cycle's increment.
- **No rewind.** The `commit_cycle` `max()` guard means a stale in-memory value,
  a concurrent CLI writer, or a hand-edited lowered file can never move the
  counter backward.
- **Fresh brain starts at 1.** With no prior state (`cycle_count == 0`, no
  reports) the first cycle increments to and displays `#1`.
- **One displayed number.** Cycle reports, `daemon_health.json`, telemetry, and
  every dashboard panel show the same brain-relative value for a given cycle. The
  one exception is the `cycle=` tracing span field, captured at cycle *entry*
  (before the per-cycle `+= 1`), which therefore trails that value by one — a
  pre-existing display detail this feature does not change (see
  [Auto-corrected derivations](#auto-corrected-derivations)).

**Not guaranteed:**

- **Strictly gap-free numbering.** A crash mid-cycle drops that cycle's
  increment, so the sequence is monotonic but may skip a number across an
  unclean restart. The counter measures *committed lived cognition*, not an
  unbroken tick sequence.
- **Cross-brain portability.** The counter is per-state-root. Copying a brain to
  a new state root carries its `goal_board.json` (and thus its count); starting
  a genuinely empty state root begins a new brain at `#1` by design.
- **`u32` ceiling.** The counter is `u32`; at the OODA cadence the ceiling is
  ~10^5 years away and is not a practical concern.

## Tests

Hermetic tests (`tempfile::tempdir()`, no network, no daemon):

> The added `commit_cycle` parameter is compiler-enforced, so the existing
> callers in `goal_board_store/tests.rs` and `operator_cli/tests_goal.rs` must be
> updated to pass a `cycle_count` argument as part of this change.

- **T1 — restart continuity.** Advance the counter, `commit_cycle`, drop the
  in-memory state, `load` again, and assert the reloaded `cycle_count` continues
  the sequence (does **not** reset to `1`).
- **T2 — monotonic `max()`.** `commit_cycle` with a value **lower** than the
  persisted one leaves the stored `cycle_count` unchanged (no rewind).
- **T3 — fresh brain starts at 1.** A fresh temp state root loads `cycle_count == 0`;
  after one increment the first `CycleReport.cycle_number` / `cycle=` field is `1`.
- **T4 — legacy file loads as 0.** A `goal_board.json` lacking the field
  deserialises with `cycle_count == 0` and does not fail the load.
- **T5 — dashboard renders brain-relative after restart.** With persisted reports
  at a high index and a freshly restarted health counter,
  `authoritative_cycle_number` returns the durable high number, not `1` (#1680).
- **T6 — `new()` invariant preserved.** `OodaState::new()` still yields
  `cycle_count == 0` (the seed is applied by the daemon, not the constructor), so
  the existing report/state unit tests stay green.
- **Backfill** — a state root with `cycle_count == 0` and `cycle_<N>.json` reports
  present seeds the counter from the highest report index on load; a state root
  with **no** reports stays at `0`; the pass is idempotent.

## What is unchanged

- `STORE_VERSION` stays `1`; the store's `flock`, atomic-rename, and corruption
  guard are untouched — one field is added inside the existing transaction.
- `OodaState::new()` still initialises `cycle_count: 0`.
- `cycles_run` still exists and still drives the `--cycles` (`max_cycles`) stop
  condition; it is simply no longer displayed.
- `CycleReport`'s struct/serialization, the `cycle=` span, and the
  `cycle_<N>.json` schema are byte-for-byte unchanged — they now carry a durable
  number because their `cycle_count` source is durable.
- `cycle_source::authoritative_cycle_number` keeps its `max()` shape; its two
  inputs now agree instead of contradicting.

## See also

- [Concept: brain-relative OODA cycle counter](../concepts/brain-relative-ooda-cycle-counter.md) — why the count must track the brain's lived cognition, not process uptime.
- [Authoritative goal-board store](../concepts/authoritative-goal-board-store.md) — the durable, `flock`-guarded `goal_board.json` this counter rides on.
- [No-progress breaker API](./no-progress-breaker-api.md) — the sibling per-goal counter persisted in the same store for the same restart-durability reason.
- [Activity tab — Cycle Reports](./dashboard-activity-cycle-reports.md) — the shared cycle-report reader and `authoritative_cycle_number` reconciliation.
- [Thinking tab — Cycle History](./dashboard-thinking-cycle-history.md) — the collapse/duration surface that renders the same durable index.
- [How to inspect the OODA cycle counter](../howto/inspect-the-ooda-cycle-counter.md) — verify durability across a restart.
