---
title: Compound objective splitting in preparation memory operations
description: How preparation_memory_operations() splits multi-goal objectives into individual search queries to avoid the Cypher CONTAINS mismatch that returns zero facts.
last_updated: 2026-06-12
owner: simard
doc_type: concept
related:
  - ./goal-fact-dedup-in-preparation.md
  - ../reference/cognitive-memory-client-helpers.md
  - ../architecture/cognitive-memory.md
  - ../memory.md
  - ../howto/diagnose-search-facts-issues.md
---

# Compound objective splitting in preparation memory operations

> **De-fork Phase 2b.** The split-search *behavior* described here lives in
> `preparation_memory_operations()` (`src/memory_consolidation/mod.rs`), which is
> unchanged and remains backend-agnostic via the `CognitiveMemoryOps` trait. The
> `search_facts()` implementation and its diagnostic logging that this page
> anchors to `src/cognitive_memory/ops.rs` moved into the `amplihack-memory-lib`
> backend (`LibraryCognitiveMemory`) when the native fork was deleted; treat the
> `ops.rs` citations below as historical. See
> [Library-backed Cognitive Memory](../architecture/cognitive-memory-library-adapter.md).

`preparation_memory_operations()` in `src/memory_consolidation/mod.rs`
assembles a `PreparedContext` for each engineer session. A key step is
querying cognitive memory for facts relevant to the session's objective.

Prior to issue [#2270](https://github.com/rysweet/Simard/issues/2270),
the function passed the **full joined objective string** to
`client.search_facts()`, which uses a Cypher `CONTAINS` predicate
internally. This caused a silent data-loss bug: no stored fact's content
matches a giant concatenated string like
`"Implement auth module; Fix CSS layout; Update README"`, so the query
always returned zero results for multi-goal sessions.

---

## The problem

When the OODA daemon dispatches an engineer for multiple goals, it joins
all goal descriptions with `"; "` into a single `objective` string:

```
"Implement auth module; Fix CSS layout; Update README"
```

The `search_facts(query, limit, threshold)` client method translates
`query` into a Cypher `CONTAINS` predicate that searches **both** the
`concept` and `content` fields:

```cypher
MATCH (f:Fact)
WHERE (f.concept CONTAINS '{q}' OR f.content CONTAINS '{q}')
  AND f.confidence >= {min_confidence}
```

The query string is sanitized via `escape_cypher()` and interpolated
directly (not parameterized with `$query`). No fact in the graph has
concept or content that contains the entire joined string. Each fact
relates to a single goal description — e.g., a fact might contain
`"auth module"` or `"CSS layout"`, but never the full semicolon-joined
concatenation. Result: **zero facts returned** for every multi-goal
preparation, silently degrading the engineer's context to empty.

Single-goal sessions were unaffected because the objective string
contained exactly one description, which could plausibly substring-match
stored facts.

---

## The fix: per-fragment search with dedup

`preparation_memory_operations()` now splits the objective on `"; "` and
searches each fragment independently:

### Algorithm

```
1. Split objective on "; " → fragments[]
   (If no delimiter found, fragments = [objective] — single element)

2. For each fragment in fragments:
     results += search_facts(fragment, 10, 0.0)

3. Deduplicate results by node_id (HashSet, keep first seen)

4. Cap total at 10 facts
```

### Why split on `"; "`

The `"; "` delimiter is the same one used by the OODA daemon when
joining goal descriptions into the objective string (see
`src/ooda_loop/cycle.rs`). Splitting on the same delimiter recovers the
original individual descriptions.

### Why deduplicate by `node_id`

Different fragments may match the same fact. For example, goals
`"Implement auth module"` and `"Fix auth tests"` could both match a
fact containing `"auth"`. Without dedup, the same fact would appear
multiple times in `relevant_facts`, wasting context budget.

Dedup uses a `HashSet<String>` tracking seen `node_id` values. The
first occurrence wins — this is arbitrary but deterministic for a given
result ordering.

### Why cap at 10

The original code used `search_facts(objective, 10, 0.0)` — a limit of
10. The per-fragment search preserves this cap: after dedup, only the
first 10 unique facts are kept. This prevents multi-goal sessions from
returning disproportionately more facts than single-goal sessions.

### Empty fragment handling

If splitting produces empty fragments (e.g., a trailing `"; "`), they
are skipped. An empty-string query would match all facts via `CONTAINS`,
which is equivalent to a wildcard — not the intended behavior.

---

## Interaction with goal-fact dedup

This change affects **step 1** of the preparation algorithm described in
[Goal fact dedup in preparation](./goal-fact-dedup-in-preparation.md).
The subsequent steps (goal-fact fetch, per-slug dedup, merge) are
unchanged. The split-search produces a better `relevant_facts` baseline,
which the goal-fact merge then supplements with any goal records not
already present.

---

## Removal of the redundant second preparation call

Prior to this fix, `src/ooda_loop/cycle.rs` contained **two** calls to
`preparation_memory_operations()`:

1. **Line ~188** — called with `objective_summary` (all goal
   descriptions joined with `"; "`)
2. **Line ~218** — called again with `cycle_objective` (a single goal
   description)

The second call was a workaround for the CONTAINS bug: since the first
call returned nothing useful for multi-goal objectives, someone added a
second call with just one goal description to get at least *some* facts.

With the split-search fix, the first call correctly finds facts for all
goals. The second call is redundant and has been removed.

---

## Diagnostic logging

`search_facts()` in `src/cognitive_memory/ops.rs` now emits
`tracing::debug!` logs at function entry and after query execution:

```
search_facts: query_len=47, is_wildcard=false
search_facts: returned 8 rows
```

See [Diagnose search_facts issues](../howto/diagnose-search-facts-issues.md)
for how to use these logs to verify the fix is working.

---

## Verification

The existing test in `src/memory_consolidation/tests.rs` exercises
single-fragment objectives (no `"; "` delimiter). These continue to pass
because single-fragment splitting produces exactly one `search_facts`
call with the same arguments as before.

Multi-goal behavior can be verified by enabling `RUST_LOG=debug` and
checking that each fragment produces a separate `search_facts` log
entry.

---

## Code location

| Item | File | Line |
|------|------|------|
| `preparation_memory_operations()` (split logic) | `src/memory_consolidation/mod.rs` | ~83 |
| Redundant call removed | `src/ooda_loop/cycle.rs` | ~218 (deleted) |
| `search_facts()` diagnostic logging | `src/cognitive_memory/ops.rs` | ~270 |

---

## Related reading

- [Goal fact dedup in preparation](./goal-fact-dedup-in-preparation.md)
  — the per-slug dedup that runs after the objective search.
- [Cognitive memory client helpers](../reference/cognitive-memory-client-helpers.md)
  — how `search_facts` reaches the graph store.
- [Memory architecture](../memory.md) — overview of the six memory
  types.
- [Diagnose search_facts issues](../howto/diagnose-search-facts-issues.md)
  — using the new diagnostic logging.
