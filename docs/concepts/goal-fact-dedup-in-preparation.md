---
title: Goal fact dedup in memory consolidation preparation
description: Why and how preparation_memory_operations() loads goal facts from cognitive memory and deduplicates them by slug before including them in the PreparedContext.
last_updated: 2026-06-10
owner: simard
doc_type: concept
related:
  - ../reference/goal-prospective-memory-mirror.md
  - ../reference/cognitive-memory-goal-store.md
  - ../architecture/cognitive-memory.md
  - ../memory.md
  - ./preparation-compound-objective-search.md
  - ../howto/diagnose-search-facts-issues.md
  - ../reference/backup-pruning-api.md
---

# Goal fact dedup in memory consolidation preparation

> **Implementation status:** This document describes the target design
> being built in issue
> [#2207](https://github.com/rysweet/Simard/issues/2207). It must be
> merged alongside the implementation code — not before.

The `preparation_memory_operations()` function in
`src/memory_consolidation/mod.rs` assembles a `PreparedContext` for each
engineer session. As of issue #2207, it explicitly loads goal-record
facts from semantic memory so that Active goals are always available in
the prepared context — regardless of whether the session objective
happens to substring-match `"goal-store:record"`.

This page explains the dedup strategy that prevents historical goal
revisions from crowding out current records.

---

## The problem

Goal records in cognitive memory are **append-only**: every `put()` call
appends a new fact with concept `"goal-store:record"`. When a goal is
updated (status change, priority change, re-prioritisation), the old
revision stays in the store and a new revision is appended. Over time a
goal with frequent updates accumulates many revisions.

Without dedup, `preparation_memory_operations()` would include all
revisions in `relevant_facts`. This has two consequences:

1. **Context pollution** — the agent sees outdated versions of the same
   goal alongside the current one, potentially acting on stale state.
2. **Truncation risk** — the `search_facts` call has a bounded result
   set. If historical revisions consume most of the limit, current goals
   can be pushed out entirely.

---

## The solution: latest-per-slug dedup

`preparation_memory_operations()` loads goal facts using the same
concept key and limit that `CognitiveMemoryGoalStore::list_via_reader()`
uses (via the shared `pub(crate)` constants `GOAL_STORE_FACT_CONCEPT`
and `GOAL_STORE_LIST_LIMIT`), then deduplicates by slug before merging
into `relevant_facts`.

### Algorithm

```
1. Fetch objective-related facts (with compound-objective splitting):
     Split objective on "; " → fragments[]
     For each fragment:
       results += search_facts(fragment, 10, 0.0)
     Deduplicate results by node_id (keep first seen)
     relevant_facts = first 10 unique results
     (See: Compound objective splitting in preparation memory operations)

2. Fetch goal facts:
     goal_facts = search_facts(GOAL_STORE_FACT_CONCEPT, GOAL_STORE_LIST_LIMIT, 0.0)

3. Dedup goal_facts by slug, keeping the largest node_id per slug:
     For each fact in goal_facts:
       - Parse fact.content as GoalRecord (skip on parse failure)
       - Group by record.slug
       - Keep the fact with the largest node_id (UUID-v7 — time-ordered)
     Result: one fact per slug, representing the current state

4. Merge deduped goal facts into relevant_facts:
     For each deduped goal fact:
       - Skip if its node_id is already in relevant_facts (from step 1)
       - Otherwise append to relevant_facts
```

### Why this mirrors `list_via_reader()`

The dedup logic is intentionally identical to
`CognitiveMemoryGoalStore::list_via_reader()`:

- Same concept key (`GOAL_STORE_FACT_CONCEPT`)
- Same limit (`GOAL_STORE_LIST_LIMIT = 256`)
- Same grouping (by `record.slug`)
- Same winner selection (largest `node_id`)
- Same error handling (skip unparseable facts with `continue`)

This guarantees that the facts in `PreparedContext.relevant_facts` match
the records returned by `GoalStore::list()` — the agent never sees a
goal revision that `list()` would not return.

### Why not call `list_via_reader()` directly

`preparation_memory_operations()` receives a `&dyn CognitiveMemoryOps`
trait object, not a `CognitiveMemoryGoalStore` instance. The function
operates at the memory-operations abstraction level and does not depend
on the goal-store module. Duplicating the dedup logic (which is a
handful of lines) avoids introducing a circular dependency or a new
trait method.

---

## Constants shared across modules

The following constants are defined in `src/goals/cognitive_memory_store.rs`
as `pub(crate)` and imported by `src/memory_consolidation/mod.rs`:

| Constant | Value | Used for |
|----------|-------|----------|
| `GOAL_STORE_FACT_CONCEPT` | `"goal-store:record"` | The concept key passed to `search_facts` |
| `GOAL_STORE_LIST_LIMIT` | `256` | The maximum number of facts retrieved |

Using shared constants ensures that if the concept key or limit changes,
both modules stay in sync.

---

## Parse failure handling

Not every fact with concept `"goal-store:record"` is guaranteed to be
valid JSON or a valid `GoalRecord`. The dedup step uses
`match serde_json::from_str` with a `continue` on `Err` — unparseable
facts are silently skipped. This matches the defensive pattern used
throughout the codebase (e.g., `list_via_reader()`,
`migrate_file_backed_goal_store_if_present()`).

---

## Limit sizing

`GOAL_STORE_LIST_LIMIT` is set to `256`. With `MAX_ACTIVE_GOALS = 5`
and typical churn (a handful of status transitions per goal), 256 raw
facts covers realistic deployments without risking truncation. The
previous hardcoded value of `20` was too small: a single goal updated
five times consumed five of the twenty slots, leaving room for only
three goals with similar churn before current records started falling
off.

---

## Interaction with the existing `existing_ids` check

After dedup, the code still checks `existing_ids` (the set of node_ids
already present in `relevant_facts` from the objective search). Both
layers are needed:

- **Per-slug dedup** removes historical revisions, keeping one fact per
  goal.
- **`existing_ids`** prevents the same fact from appearing twice if the
  objective search already found it.

The per-slug dedup runs first because it reduces the candidate set
before the `existing_ids` check, which is a simple `HashSet::contains`.

---

## Code location

| Item | File | Line |
|------|------|------|
| `preparation_memory_operations()` | `src/memory_consolidation/mod.rs` | ~77 |
| `GOAL_STORE_FACT_CONCEPT` | `src/goals/cognitive_memory_store.rs` | ~30 |
| `GOAL_STORE_LIST_LIMIT` | `src/goals/cognitive_memory_store.rs` | ~43 |
| `GoalRecord` (imported for dedup parse) | `src/goals/mod.rs` | re-exported |

---

## Related reading

- [Compound objective splitting](./preparation-compound-objective-search.md)
  — how the objective search in step 1 splits multi-goal objectives
  into per-fragment queries.
- [Goal–prospective memory mirror](../reference/goal-prospective-memory-mirror.md)
  — the dual-write mechanism that creates goal facts and prospective
  entries.
- [Cognitive memory goal store](../reference/cognitive-memory-goal-store.md)
  — historical design context.
- [Memory architecture](../memory.md) — overview of the six memory
  types.
- [Cognitive Memory Architecture](../architecture/cognitive-memory.md)
  — full schema and consolidation rules.
- [Diagnose search_facts issues](../howto/diagnose-search-facts-issues.md)
  — using diagnostic logging to verify preparation queries.
- [Backup pruning API](../reference/backup-pruning-api.md) — automatic
  retention limit for cognitive memory backups.
