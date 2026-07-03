---
title: Goal-board prospective reconcile
description: How the OODA daemon mirrors live GoalBoard active goals into prospective memory each cycle so that triggers actually fire during preparation — the board-sourced reconcile step that closes the gap left by CognitiveMemoryGoalStore never running in the daemon (issue #2308).
last_updated: 2026-06-20
owner: simard
doc_type: reference
related:
  - ./prospective-trigger-firing.md
  - ./goal-prospective-memory-mirror.md
  - ./goal-board-api.md
  - ./simard-memory-cli.md
  - ../architecture/cognitive-memory-library-adapter.md
  - ../memory.md
---

# Goal-board prospective reconcile

> Shipped in issue [#2308](https://github.com/rysweet/Simard/issues/2308)
> follow-up (fix trigger population on the library backend). Companion to
> the read-side [Prospective-trigger firing](./prospective-trigger-firing.md)
> and the introspection [Memory CLI](./simard-memory-cli.md).

Active goals must surface as prospective-memory **triggers** during the
OODA preparation phase. Before #2308 they never did: with five active
goals the daemon still logged `0 triggers` every cycle. This page
documents the **write side** that closes that gap — a per-cycle
**board-sourced reconcile** that mirrors the live `GoalBoard` into
prospective memory using the library backend's `store_prospective`.

---

## The defect this fixes

Every preparation pass calls `check_triggers(objective)` and surfaces the
matches as `PreparedContext.triggered_prospectives`. For a match to
exist, a `pending` prospective with the right `trigger_condition` must
already be in the store. The
[prospective-trigger firing fix](./prospective-trigger-firing.md) (#2300)
correctly shaped the *read* probe so a stored trigger would match — but in
the live daemon **nothing was writing the triggers in the first place**:

```
[simard] OODA cycle: prepared context (5 facts, 0 triggers, 5 procedures, 0 episodes)
                                                  ^^^^^^^^^^ permanent, despite goals=5 active
```

### Root cause — the two goal systems never met

Simard has two goal-storage paths:

| Path | File | Writes prospects? | Used by the daemon? |
|------|------|-------------------|---------------------|
| File/board snapshot | `src/goal_curation/*`, `src/goals/store.rs` | no | **yes** — the daemon loads/persists the live `GoalBoard` snapshot (`goal-board:snapshot` facts) |
| Cognitive-memory goal store | `src/goals/cognitive_memory_store.rs` (`CognitiveMemoryGoalStore`) | yes — `put(Active)` calls `store_prospective` | **no** — never invoked by the daemon |

The only live prospective writer, `CognitiveMemoryGoalStore::put()`, is
**never called by the running daemon**. Its sibling
`reconcile_prospectives()` reads `goal-store:record` facts that the daemon
never writes, and is only exercised from tests. So Active goals lived
entirely in the `GoalBoard` snapshot and were never mirrored into
prospective memory — `check_triggers` had nothing to match, and
`prospective_count` stayed at zero (now directly observable with
[`simard memory stats`](./simard-memory-cli.md)).

### Why not just swap the daemon to `CognitiveMemoryGoalStore`?

Migrating the daemon's goal persistence from the `GoalBoard` snapshot to
`CognitiveMemoryGoalStore` would be a broad, risky refactor touching goal
curation, persistence, and recovery — out of scope for a population fix,
and a likely source of regressions in the board's corruption guards and
backlog semantics. The fix is deliberately **surgical**: keep the
`GoalBoard` as the source of truth and add a thin mirror step.

---

## The fix — a per-cycle board-sourced reconcile

A reconcile step runs **before preparation, every OODA cycle**. For each
Active goal in the live `GoalBoard` it ensures a `pending` prospective
exists in the library store, written through the daemon's own memory
bridge:

```
for goal in goal_board.active:
    trigger_condition = goal_slug(goal.id).replace('-', ' ')   // slug-phrase
    if no pending prospective with this trigger_condition:
        adapters.memory.store_prospective(
            description       = "goal:{goal.description}",
            trigger_condition = trigger_condition,
            action_on_trigger = "Pursue goal: …",
            priority          = goal.priority)
```

The reconcile reuses the two proven pieces from the existing mirror so the
write and read sides stay byte-identical:

- The **slug-phrase transform** `goal_slug(id).replace('-', ' ')` — the
  same one `prospective_trigger_for()` uses on the write side and
  `build_objective_probe()` uses on the read side (see the
  [slug-phrase invariant](./prospective-trigger-firing.md#the-slug-phrase-invariant)).
- The **`goal:` description prefix** (`GOAL_PROSPECTIVE_PREFIX`) so the
  reconcile can recognise and dedup its own prospects without disturbing
  non-goal prospects (meeting action items, etc.).

### Placement: every cycle, before preparation

The reconcile is **idempotent** and runs **before** the preparation pass
that calls `check_triggers`, so a freshly-added goal fires on the very
next cycle rather than the one after. Idempotency matters because the
library's `check_triggers` marks a matched prospective `"triggered"`
(fire-once); re-running the reconcile each cycle re-establishes a
`pending` prospective for every still-active goal, so the trigger keeps
firing as long as the goal stays active:

```
cycle N:   reconcile → ensure pending prospect for each active goal
           preparation → check_triggers(probe) → match → mark "triggered"
cycle N+1: reconcile → goal still active, no pending prospect → recreate pending
           preparation → check_triggers(probe) → matches again
```

Without the per-cycle re-establish, a goal would fire exactly once and
then go quiet — defeating the purpose. The reconcile is therefore the
counterpart to fire-once semantics, not a duplicate of it.

### Resolving prospects for goals that leave the board

When a goal is no longer Active (completed, demoted, removed), the
reconcile resolves its lingering `goal:`-prefixed prospects so the store
does not accumulate stale triggers. This mirrors
`resolve_goal_prospectives()` from the
[goal-prospective mirror](./goal-prospective-memory-mirror.md#resolve_goal_prospectives)
but is driven by the live board rather than `goal-store:record` facts.

---

## End-to-end flow

```
GoalBoard (live, daemon source of truth)
  active = [ "fix-episode-recall" (p1), … ]
        │
        ▼  (each OODA cycle, before preparation)
board-sourced reconcile
  └─ store_prospective(
        description       = "goal:Fix episode recall",
        trigger_condition = "fix episode recall",     ← slug-phrase
        action_on_trigger = "Pursue goal: …",
        priority          = 1)                          (pending)
        │
        ▼  (preparation, same cycle)
build_objective_probe(active)
  = "… Fix episode recall fix episode recall; …"        ← contains needle
        │
        ▼
check_triggers(probe)
  → 1 pending prospective matches → PreparedContext.triggered_prospectives
        │
        ▼
[simard] OODA cycle: prepared context (5 facts, 1 triggers, 5 procedures, 0 episodes)
                                                ^^^^^^^^^^ non-zero after the fix
```

Operator-visible confirmation with the
[introspection CLI](./simard-memory-cli.md):

```text
$ simard memory stats
  prospective       5     (triggers)       ← first cycle: one pending per active goal
```

> **The `prospective` count is a floor, not a fixed gauge.** On the first
> reconcile against an empty store it equals the active-goal count (five
> goals → five pending prospects). Across later cycles it only **grows**:
> `check_triggers` marks each fired prospect `"triggered"` (fire-once, never
> deleted), and the next reconcile creates a fresh `pending` prospect for
> the still-active goal — so the *total* prospective node count climbs cycle
> over cycle even with a stable goal set. Treat `prospective_count > 0` (and
> non-decreasing) as "triggers are populating"; the meaningful per-cycle
> "fired" signal is the cycle-log `N triggers` figure, not this total. See
> [Memory introspection CLI → Triggers](./simard-memory-cli.md#triggers-confirming-goal-population).

---

## Acceptance criterion

> With at least one Active goal on the board, an OODA preparation pass
> must report `triggers > 0`.

The TDD red for this fix seeds an Active goal, runs the reconcile +
preparation path, and asserts a trigger fires. It fails on the
pre-#2308-follow-up daemon (`0 triggers`) and passes once the board-sourced
reconcile lands.

---

## Code location

| Item | File |
|------|------|
| Board-sourced reconcile step | `src/memory_consolidation/mod.rs` (preparation entry) |
| Live consumer (`check_triggers(objective)`) | `src/memory_consolidation/mod.rs` |
| `store_prospective` (write) | `src/cognitive_memory/library_adapter.rs` |
| Slug-phrase transform / `GOAL_PROSPECTIVE_PREFIX` | `src/goals/cognitive_memory_store.rs` |
| Objective probe (`build_objective_probe`) | `src/ooda_loop/cycle.rs` |
| Live `GoalBoard` load/persist | `src/goal_curation/operations.rs` |
| Daemon memory bridge wiring | `src/operator_commands_ooda/daemon/mod.rs` |

---

## Testing

| Test | Coverage |
|------|----------|
| Active goal → trigger fires (the TDD red) | Seed one Active goal in a `GoalBoard`, run reconcile + preparation, assert `triggered_prospectives` is non-empty |
| Reconcile is idempotent across cycles | Run reconcile twice; assert exactly one pending prospect per active goal (no duplicates) |
| Fire-once then re-establish | Trigger fires (marked `triggered`), next reconcile recreates a pending prospect, trigger fires again |
| Goal leaves the board → prospect resolved | Mark a previously-active goal inactive, run reconcile, assert its `goal:` prospect is resolved and no longer fires |
| `prospective_count` reflects active goals | After the **first** reconcile against an empty store, `get_statistics().prospective_count` equals the active-goal count; across later cycles it only grows (fired prospects are marked `triggered`, not deleted — see [Memory CLI](./simard-memory-cli.md#triggers-confirming-goal-population)). Assert the first-reconcile equality, and that a second cycle does not *decrease* the count |

---

## Out of scope

- **Migrating the daemon to `CognitiveMemoryGoalStore`.** The fix keeps
  the `GoalBoard` as the source of truth and mirrors from it. Unifying the
  two goal systems is a separate, larger refactor.
- **Non-goal prospects.** Meeting action items and other prospects are
  untouched — the reconcile only manages `goal:`-prefixed entries.
- **The read/match itself.** Probe shaping and the case/keyword match are
  owned by [Prospective-trigger firing](./prospective-trigger-firing.md)
  (#2300); this fix supplies the prospects that fix matches.
- **Retention / compaction of fired prospects.** Because fire-once leaves
  each matched prospect as a `"triggered"` node and the per-cycle reconcile
  re-creates a fresh `pending` one, the total prospective node count grows
  unboundedly for a long-lived daemon (one new node per active goal per
  firing cycle). This is acceptable for a population fix — `prospective_count`
  is a monotonic floor, not a steady-state gauge — but a future change may
  want to compact or resolve stale `triggered` nodes. Not addressed here.

---

## Related reading

- [Prospective-trigger firing](./prospective-trigger-firing.md) — the
  read side: how the objective probe is shaped and how the match fires.
- [Goal-prospective memory mirror](./goal-prospective-memory-mirror.md) —
  the original `CognitiveMemoryGoalStore` mirror this reconcile reuses the
  slug-phrase and prefix conventions from.
- [Memory introspection CLI](./simard-memory-cli.md) — confirm the
  `prospective` count with `simard memory stats`.
- [Goal board API](./goal-board-api.md) — the live `GoalBoard` the
  reconcile reads from.
- [Memory architecture](../memory.md) — the six memory types and the
  consolidation flow.
