---
title: Goal–prospective memory mirror
description: API reference for the prospective-memory mirror in CognitiveMemoryGoalStore — how put() dual-writes prospective entries for Active goals, error propagation, reconcile_prospectives(), and shared constants.
last_updated: 2026-06-12
owner: simard
doc_type: reference
related:
  - ./cognitive-memory-goal-store.md
  - ./goal-board-api.md
  - ./ooda-procedural-memory.md
  - ../concepts/goal-board-persistence.md
  - ../howto/reconcile-goal-prospective-drift.md
  - ../memory.md
---

# Goal–prospective memory mirror

> Shipped in issue [#2207](https://github.com/rysweet/Simard/issues/2207)
> and extended in [#2280](https://github.com/rysweet/Simard/issues/2280)
> (test coverage for the prospective mirror).

When `CognitiveMemoryGoalStore::put()` writes a goal record to semantic
memory, it also maintains a **mirror** in prospective memory so that
Active goals surface via `check_triggers` during the OODA preparation
phase (issue #2207). This page documents the mirror's write contract,
error propagation, reconciliation API, and the constants shared between
`cognitive_memory_store.rs` and `memory_consolidation/mod.rs`.

---

## Overview

The goal store uses a dual-write strategy:

1. **Primary write** — the `GoalRecord` is serialised as a
   `goal-store:record` fact in semantic memory (append-only,
   latest-per-slug dedup on read).
2. **Mirror write** — if the record's status is `Active`, a prospective
   memory entry is stored so that `check_triggers` fires when the OODA
   objective summary mentions related terms.

Both writes occur inside a single `put()` call using the same
`WriterBridge`. If either the primary fact write or the mirror write
fails, `put()` returns `Err` — callers see the error and can retry or
surface it.

```
put(record)
  │
  ├─ store_fact("goal-store:record", json(record))     ← primary
  ├─ resolve_goal_prospectives(slug)                    ← clear stale
  └─ if Active:
       store_prospective(description, trigger, action)  ← mirror
```

---

## Error propagation contract

Both `resolve_goal_prospectives()` and `store_prospective()` propagate
errors via `?`. This means:

- If the primary `store_fact` succeeds but `resolve_goal_prospectives`
  fails, `put()` returns `Err`. The fact is already appended (append-only
  store), so a retry of `put()` appends a second revision — harmless
  because `list()` deduplicates by slug, keeping only the latest.
- If the primary `store_fact` and `resolve_goal_prospectives` both
  succeed but `store_prospective` fails, `put()` returns `Err`. The goal
  exists in semantic memory but has no prospective trigger. Callers can
  retry, or the drift can be fixed later by `reconcile_prospectives()`.

This is safe because `put()` is idempotent: repeated calls for the same
slug append new revisions, and `list_via_reader()` always keeps only the
latest per slug.

---

## `reconcile_prospectives()`

```rust
impl CognitiveMemoryGoalStore {
    /// Walk all current goal records and ensure prospective memory
    /// mirrors are consistent with goal state.
    ///
    /// - Active goals: ensure a prospective entry exists.
    /// - Non-Active goals (Completed, Paused, Proposed): resolve any
    ///   stale prospective entries.
    ///
    /// Opens its own bridges internally. Returns the first error
    /// encountered — callers can retry or log as appropriate.
    pub fn reconcile_prospectives(&self) -> SimardResult<()>;
}
```

### Behaviour

1. Calls `list_via_reader()` to load all current (latest-per-slug) goal
   records.
2. Opens a `WriterBridge` for prospective operations.
3. For each record:
   - **Active**: calls `check_triggers` with the slug-derived trigger
     phrase. If no matching prospective entry exists, calls
     `store_prospective` to create one.
   - **Non-Active**: calls `resolve_goal_prospectives` to mark any
     lingering prospective entries as resolved.
4. Returns `Ok(())` on full success, or the first `Err` encountered.

> **Partial reconciliation:** Because the method returns on the first
> error, goals after the failing one are not processed. Callers should
> retry on error to fix remaining items. If this proves problematic in
> practice, a future revision may collect all errors and return a summary
> instead of short-circuiting.

### When to call

- **Periodically** — e.g., at OODA cycle start or during a health check
  — to fix any drift caused by transient bridge failures during prior
  `put()` calls.
- **After recovery** — after restoring a cognitive memory database from
  backup.
- **Never required for correctness** in the happy path — `put()`
  propagates errors, so callers know when the mirror is inconsistent.

---

## Constants

These constants are `pub(crate)` so that both `cognitive_memory_store.rs`
and `memory_consolidation/mod.rs` share a single definition:

| Constant | Value | Purpose |
|----------|-------|---------|
| `GOAL_STORE_FACT_CONCEPT` | `"goal-store:record"` | Concept key for all goal-record facts in semantic memory |
| `GOAL_STORE_LIST_LIMIT` | `256` | Maximum facts fetched by `search_facts` for goal reads |
| `GOAL_STORE_SOURCE` | `"goal-store"` | Source label recorded with every fact |
| `GOAL_STORE_TAG` | `"goal-store"` | Tag recorded with every fact |
| `GOAL_PROSPECTIVE_PREFIX` | `"goal:"` | Description prefix that distinguishes goal prospective entries from other prospective entries (e.g., meeting action items) |

`GOAL_STORE_FACT_CONCEPT` and `GOAL_STORE_LIST_LIMIT` are used by
`preparation_memory_operations()` in `memory_consolidation/mod.rs` to
load and deduplicate goal facts. See
[Goal fact dedup in preparation](../concepts/goal-fact-dedup-in-preparation.md).

---

## Prospective entry format

Each Active goal produces one prospective memory entry:

| Field | Value | Example |
|-------|-------|---------|
| `description` | `"goal:{title}"` | `"goal:Fix broken features"` |
| `trigger_condition` | slug with dashes → spaces | `"fix broken features"` |
| `action_on_trigger` | `"Pursue goal: {title} (p{priority}, {rationale})"` | `"Pursue goal: Fix broken features (p1, CI is red)"` |
| `importance` | `record.priority` as `i64` | `1` |

The `trigger_condition` uses the slug with dashes replaced by spaces so
that substring matching in `check_triggers` fires when the OODA
objective summary mentions similar terms.

---

## `resolve_goal_prospectives()`

```rust
fn resolve_goal_prospectives(
    slug: &str,
    ops: &dyn CognitiveMemoryOps,
) -> SimardResult<()>;
```

Resolves (marks as `"resolved"`) any pending prospective memories whose
description starts with `GOAL_PROSPECTIVE_PREFIX` and whose
`trigger_condition` exactly matches the slug-derived trigger phrase. This
prevents accumulation of stale prospective entries when a goal is re-put
or transitions to `Completed`/`Paused`.

Called by `put()` before every mirror write, and by
`reconcile_prospectives()` for non-Active goals.

---

## Code location

| Item | File |
|------|------|
| `CognitiveMemoryGoalStore` | `src/goals/cognitive_memory_store.rs` |
| `reconcile_prospectives()` | `src/goals/cognitive_memory_store.rs` |
| `resolve_goal_prospectives()` | `src/goals/cognitive_memory_store.rs` |
| `prospective_trigger_for()` | `src/goals/cognitive_memory_store.rs` |
| Constants | `src/goals/cognitive_memory_store.rs` |
| Preparation phase consumer | `src/memory_consolidation/mod.rs` |

---

## Related reading

- [Goal fact dedup in preparation](../concepts/goal-fact-dedup-in-preparation.md)
  — how `preparation_memory_operations` loads and deduplicates goal
  facts using the shared constants.
- [How to reconcile goal–prospective drift](../howto/reconcile-goal-prospective-drift.md)
  — operator guide for detecting and fixing inconsistencies.
- [Cognitive memory goal store (historical)](./cognitive-memory-goal-store.md)
  — historical design context for the adapter.
- [Goal board persistence](../concepts/goal-board-persistence.md)
  — the broader persistence lifecycle.
- [Memory architecture](../memory.md) — the six memory types.
