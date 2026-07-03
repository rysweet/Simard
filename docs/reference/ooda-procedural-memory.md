---
title: OODA procedural memory
description: How the OODA cycle stores successful action outcomes as procedural memories, enabling Simard to recall what worked in past cycles during future preparation phases.
last_updated: 2026-07-03
owner: cognitive-memory
doc_type: reference
related:
  - ../architecture/cognitive-memory.md
  - ./cognitive-memory-ranked-episodic-recall.md
  - ./cognitive-memory-procedural-idempotency.md
  - ./procedural-learning-loop.md
  - ../concepts/procedural-learning-loop.md
  - ../howto/inspect-the-procedural-learning-loop.md
  - ./goal-prospective-memory-mirror.md
  - ./ooda-brain-api.md
  - ./ooda-engineer-lifecycle-recipe.md
  - ../memory.md
---

# OODA procedural memory

> **De-fork Phase 2b (#2307).** The store/recall *behavior* described here
> is preserved: `store_procedure` and `recall_procedure` are reached through the
> `CognitiveMemoryOps` trait, now backed solely by `LibraryCognitiveMemory` over
> the external `amplihack-memory` library. Implementation citations to
> `src/cognitive_memory/ops.rs` (`NativeCognitiveMemory`) and tests that exercised
> `NativeCognitiveMemory` directly were deleted with the fork; treat those code
> citations as historical. See
> [Library-backed Cognitive Memory](../architecture/cognitive-memory-library-adapter.md).

Shipped in issue [#2280](https://github.com/rysweet/Simard/issues/2280).

> **Reinforcement enabled by #2395.** When #2280 shipped, a procedure's
> `usage_count` only moved at **store** time (and, since #2298, on exact-name
> re-store). Nothing incremented it when a procedure was **recalled and used**,
> so the usage signal the ranker reads stayed flat. #2395 adds a single
> reinforce-at-use seam (`reinforce_access`), driven from the goal-session path
> (`advance.rs`) for every recalled procedure surfaced into a cycle's prompt
> (per-action attribution — only the procedure that drove the action — is a
> future refinement), and clarifies that preparation procedure recall is
> **usage-ordered** (the library sorts by `usage_count` descending and matches
> name **OR** steps — not the newest-first single-`CONTAINS`-on-name scan once
> described here). See
> [Ranked episodic recall & memory reinforcement](./cognitive-memory-ranked-episodic-recall.md).

After the OODA Act phase dispatches actions and collects outcomes, each
**successful** outcome is stored as a procedural memory. This enables
Simard to recall effective action sequences in future OODA cycles via
`recall_procedure` during the preparation phase.

---

## Overview

Procedural memory is the cognitive memory type for "how-to" knowledge —
named, reusable step sequences that encode what worked. Before issue
#2280, procedural memories were never written by any production code
path. The `store_procedure` API existed and was tested at the unit
level, but no caller invoked it. This left the procedural memory table
permanently empty.

The OODA cycle's Act phase is the natural site for procedural learning:
it dispatches actions, observes outcomes, and knows which succeeded.
After execution, the cycle now iterates over outcomes and stores each
successful one as a procedure.

```
Act phase
  │
  ├─ dispatch actions → outcomes[]
  ├─ execution_memory_operations(outcome.detail)   ← existing
  ├─ store_procedure(outcome) for each success      ← NEW (#2280)
  │
  └─ review_outcomes(outcomes)
```

---

## Procedure naming convention

> **Superseded since #2281.** The live OODA write path now derives procedure
> names via `compose_procedure_name` (goal-scoped, trigger-bearing — e.g.
> `pr-merge:{goal_id} | triggers: merge,pr,review`), **not** the
> `ooda:{action_kind}` form shown below. That full name string is also the
> dedup key for #2298 idempotency. See
> [Bootstrap procedures and trigger naming](./cognitive-memory-bootstrap-procedures.md)
> and
> [Procedural-memory store idempotency](./cognitive-memory-procedural-idempotency.md).
> The original #2280 scheme below is retained for historical context.

Each procedure was originally named `ooda:{action_kind}` (the #2280 scheme),
where `action_kind` is the `Display` representation of the `ActionKind` enum:

| ActionKind             | Procedure name              |
|------------------------|-----------------------------|
| `AdvanceGoal`          | `ooda:advance-goal`         |
| `RunImprovement`       | `ooda:run-improvement`      |
| `ConsolidateMemory`    | `ooda:consolidate-memory`   |
| `ResearchQuery`        | `ooda:research-query`       |
| `RunGymEval`           | `ooda:run-gym-eval`         |
| `BuildSkill`           | `ooda:build-skill`          |
| `LaunchSession`        | `ooda:launch-session`       |
| `PollDeveloperActivity`| `ooda:poll-developer-activity` |
| `ExtractIdeas`         | `ooda:extract-ideas`        |
| `SafeUpdate`           | `ooda:safe-update`          |

The `ooda:` prefix distinguishes OODA-learned procedures from any
future manually-imported or meeting-derived procedures.

**Accumulation semantics (updated by #2298)**: when the same
`ActionKind` succeeds in multiple cycles and produces the **same
procedure name**, `store_procedure` is now idempotent on exact name —
no duplicate node is created (it bumps `usage_count` instead). When a
success produces a **distinct** name, a new procedure node is created
and accumulates. `recall_procedure` returns the de-duplicated set (it
does not currently rank by `usage_count` — rows come back in store
order under its `CONTAINS`/`LIMIT` query), so distinct learned
procedures remain queryable. See
[Procedural-memory store idempotency](./cognitive-memory-procedural-idempotency.md).
Prior to #2298 every store created a new node even for identical names,
which froze the procedural store at 0% compression.

---

## Procedure content

Each stored procedure contains two steps:

| Step index | Content | Source |
|------------|---------|--------|
| 0 | `outcome.action.description` | What was **planned** — the natural-language description from the Decide phase |
| 1 | `outcome.detail` | What **happened** — the execution output captured during Act |

Prerequisites are empty (`&[]`) because OODA actions are self-contained
dispatches — the cycle handles sequencing and precondition checking.

### Example

After a successful `AdvanceGoal` action:

```rust
store_procedure(
    "ooda:advance-goal",
    &[
        "Advance goal 'fix-auth-bug': spawn engineer to fix the null check in auth.rs".into(),
        "Engineer session completed: edited auth.rs:42, all tests pass, committed abc1234".into(),
    ],
    &[],  // no prerequisites
)
```

This produces a `Procedure` node:

```
Procedure {
    name: "ooda:advance-goal",
    steps: [
        "Advance goal 'fix-auth-bug': spawn engineer to fix the null check in auth.rs",
        "Engineer session completed: edited auth.rs:42, all tests pass, committed abc1234",
    ],
    prerequisites: [],
    usage_count: 0,
}
```

---

## When procedures are NOT stored

- **Failed outcomes** (`outcome.success == false`): failed actions
  represent "what doesn't work" — this is negative knowledge, not
  procedural memory. Failed outcomes are still captured in episodic
  memory via the existing `execution_memory_operations` and
  `reflection_memory_operations` calls.

- **Bridge/memory errors**: if `store_procedure` fails, the error is
  logged to stderr and the cycle continues. Memory failures never crash
  the OODA loop. This matches the best-effort pattern used by every
  other memory call in `cycle.rs`:

  ```
  [simard] OODA consolidation: procedural memory failed: <error>
  ```

---

## Recall during preparation

Procedural memories are available to future OODA cycles through
`recall_procedure(query, limit)` during the preparation phase. The
query is matched against procedure names and step content using
keyword-based search with n-gram reranking (the same algorithm as
`search_facts`).

Typical preparation usage:

```rust
let procedures = memory.recall_procedure("advance-goal", 5)?;
for proc in &procedures {
    println!("Procedure: {} ({} uses)", proc.name, proc.usage_count);
    for (i, step) in proc.steps.iter().enumerate() {
        println!("  {}: {}", i, step);
    }
}
```

This enables the Orient and Decide phases to consider past successful
approaches when selecting actions for the current cycle.

---

## Code location

| Item | File |
|------|------|
| Procedural storage loop | `src/ooda_loop/cycle.rs` (after `execution_memory_operations` loop) |
| `store_procedure` trait method | `src/cognitive_memory/mod.rs` (`CognitiveMemoryOps`) |
| `store_procedure` implementation | `src/cognitive_memory/library_adapter.rs` (`LibraryCognitiveMemory`) |
| `recall_procedure` implementation | `src/cognitive_memory/library_adapter.rs` (`LibraryCognitiveMemory`) |
| `ActionOutcome` / `ActionKind` types | `src/ooda_loop/types.rs` |
| Mock handlers | `tests/ooda.rs`, `tests/ooda_daemon.rs`, `tests/memory_consolidation_lifecycle.rs` |

---

## Testing

### Unit test

`successful_outcome_stores_procedural_memory` in the OODA test suite
verifies that after a cycle containing a successful outcome, the mock
transport receives a `memory.store_procedure` call (or when using
`LibraryCognitiveMemory` directly, that `get_statistics().procedural_count`
increases).

### Mock transport handler

All OODA-related integration tests include a `memory.store_procedure`
handler in their mock transport to prevent spurious `-32601 Method not
found` errors:

```rust
"memory.store_procedure" => Ok(json!({"id": "proc_new"})),
```

This handler is present in:
- `tests/ooda.rs`
- `tests/ooda_daemon.rs`
- `tests/memory_consolidation_lifecycle.rs`

---

## Relationship to other memory types

| Memory type | What it captures from OODA | When |
|-------------|---------------------------|------|
| **Sensory** | Raw PTY output, objective text | Observation phase |
| **Working** | Current goal, plan, execution state | Throughout cycle |
| **Episodic** | Full session transcripts, action logs | Reflection phase |
| **Semantic** | Extracted facts about codebase, patterns | Reflection phase |
| **Procedural** | Successful action sequences | After Act phase (**#2280**) |
| **Prospective** | Active goals as future triggers | Goal store `put()` (**#2207/#2280**) |

Before issue #2280, the procedural and prospective rows were
effectively dead — the storage APIs existed but no production code
called them from the OODA loop. Issue #2280 closes both gaps.

---

## Related reading

- [Cognitive Memory Architecture](../architecture/cognitive-memory.md) —
  the six memory types and their lifecycle.
- [Goal–prospective memory mirror](./goal-prospective-memory-mirror.md) —
  the companion fix for prospective memory (goals as triggers).
- [Memory architecture (operator summary)](../memory.md) — top-level
  memory overview.
- [OODA Brain API](./ooda-brain-api.md) — the brain that drives
  Orient/Decide/Act.
