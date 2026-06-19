---
title: Preparation-phase memory filters
description: How the OODA preparation phase filters stale and redundant facts (goal-board:snapshot revisions, stale goal-store:record entries) out of the prepared context delivered to the brain.
last_updated: 2026-06-14
owner: simard
doc_type: reference
related:
  - ../architecture/cognitive-memory.md
  - ./goal-prospective-memory-mirror.md
  - ./prospective-trigger-firing.md
  - ./ooda-procedural-memory.md
  - ./cognitive-memory-episodic-recall.md
  - ../memory.md
---

# Preparation-phase memory filters

> Shipped in issue [#2281](https://github.com/rysweet/Simard/issues/2281)
> as PR-A (retrieval cleanup). Companion to PR-B (episode distillation)
> and PR-C (procedural seeding + episodic recall).

`preparation_memory_operations` is the function that assembles the
`PreparedContext` delivered to the brain at the start of each OODA
cycle. Two classes of low-value facts used to flood that context:

1. **Goal-board snapshot revisions** — every cycle's
   `goal-board:snapshot` write produced a new revision, and all
   revisions matched the per-fragment `search_facts` call. A single
   preparation pass could surface 8–10 near-identical JSON blobs out
   of a 16-fact budget.
2. **Stale `goal-store:record` facts** — old test meetings and retired
   goals left `goal-store:record` rows behind. Slug-based dedup kept
   only the latest revision per slug, but the *stale slugs themselves*
   still surfaced on every cycle as low-value noise.

PR-A adds two filters inside `preparation_memory_operations`:

- Drop every fact whose `concept == "goal-board:snapshot"`.
- Drop every `goal-store:record` fact whose slug is not on the **live**
  goal-board (active or backlog).

The live snapshot is already injected into the prompt by
`advance.rs`, so removing snapshot facts from preparation costs the
brain nothing and frees the fact budget for genuinely diverse facts.

---

## Filter contract

### Inputs

`preparation_memory_operations` takes:

| Parameter      | Type                | Source                                                                 |
|----------------|---------------------|------------------------------------------------------------------------|
| `objective`    | `&str`              | Current cycle objective (from `state.last_objective`)                  |
| `session_id`   | `&str`              | Current session id                                                     |
| `bridge`       | `&dyn CognitiveMemoryOps` | Cognitive memory handle from `OodaBridges`                       |
| `active_slugs` | `&HashSet<&str>`    | **New in PR-A** — union of `state.active_goals.active` and `.backlog` ids |

The single caller (`src/ooda_loop/cycle.rs`, in `prepare_phase`)
constructs `active_slugs` from the live goal-board state immediately
before the call:

```rust
let active_slugs: HashSet<&str> = state
    .active_goals
    .active
    .iter()
    .map(|g| g.id.as_str())
    .chain(state.active_goals.backlog.iter().map(|b| b.id.as_str()))
    .collect();

let prepared = preparation_memory_operations(
    &state.last_objective,
    &session_id,
    &*bridges.memory,
    &active_slugs,
)?;
```

### Outputs

`PreparedContext` keeps the same public shape; only the contents of
`relevant_facts` change:

```rust
pub struct PreparedContext {
    pub relevant_facts: Vec<Fact>,            // filtered (PR-A)
    pub triggered_prospectives: Vec<Trigger>, // unchanged
    pub recalled_procedures: Vec<Procedure>,  // unchanged in PR-A; populated by PR-C's bootstrap + naming changes
    pub episodic_recall: Vec<CognitiveEpisode>, // added by PR-C, not PR-A — listed here for the complete final shape
}
```

PR-A only adds the two filters described below; the
`episodic_recall` field lands in PR-C. The struct is shown in its
final post-PR-C shape so callers reading either doc see the same
layout.

### Filter order

The two filters apply *after* the per-fragment `search_facts` calls
and *before* the existing slug-dedup pass:

```
search_facts(per fragment, limit=10) ─┐
                                      ├─► raw_facts
search_facts(GOAL_STORE_FACT_CONCEPT, ┘
              limit=256)

raw_facts
  │
  ├─ drop where concept == "goal-board:snapshot"    ← PR-A filter 1
  ├─ dedup goal-store:record by slug (existing)
  ├─ drop goal-store:record where slug ∉ active_slugs ← PR-A filter 2
  │
  └─► relevant_facts
```

Filter 1 runs first because it eliminates a large class of facts
before any further work. Filter 2 runs after the existing dedup so
that the "keep latest revision per slug" semantics are preserved for
active slugs.

---

## What gets dropped

### `goal-board:snapshot` facts

**Every** fact whose `concept` field equals `"goal-board:snapshot"`
is dropped, regardless of timestamp, source, or content. The live
goal-board state is already injected into the prompt by
`src/ooda_actions/goal_session/advance.rs` under the
`## Current Goals` section, so the snapshot facts are pure
redundancy at preparation time.

The snapshot facts remain in semantic memory — they are not deleted
from storage. The dashboard and any consumer that queries
`search_facts("goal-board:snapshot", ...)` directly still sees them.

### Stale `goal-store:record` facts

A `goal-store:record` fact is "stale" when its slug is not present in
`active_slugs`. Stale records are dropped from the prepared context.

The slug is **not** a column on the `CognitiveFact` schema. Each
`goal-store:record` fact's `content` is a JSON-serialised
`GoalRecord` (see `src/goals/cognitive_memory_store.rs`), and PR-A
parses the slug from that JSON exactly the way the existing
slug-dedup loop already does
(`src/memory_consolidation/mod.rs:117-141`):

```rust
let record: GoalRecord = match serde_json::from_str(&fact.content) {
    Ok(r) => r,
    Err(e) => {
        eprintln!(
            "[simard] preparation: skipping unparseable goal fact \
             (node_id={}): {e}",
            fact.node_id
        );
        continue;
    }
};
let slug = record.slug;
```

Unparseable facts are logged and skipped (matching the existing
behaviour), neither retained nor dropped by the stale-slug filter —
they simply do not reach it.

Examples of stale records that are filtered:

- Test meeting records like `adopt-tdd`, `decision-use-rust` that
  were never promoted to real goals.
- Records for goals that have since been completed and removed from
  the active and backlog lists.
- Records for goals that were rejected during curation.

Active and backlog records pass through. The semantics of "active"
and "backlog" follow the live `GoalBoard` state, not the snapshot
facts in semantic memory — this is the entire point of the fix.

---

## What is preserved

The filters are deliberately narrow. Anything **not** matching one of
the two rules above is untouched:

- All non-snapshot, non-goal-store facts (e.g. `pr-pattern`,
  `bug-pattern`, `lesson-learned`, `commit-summary`, `code-pattern`)
  pass through unchanged.
- `goal-store:record` facts for slugs that *are* on the active or
  backlog list pass through after the existing slug-dedup.
- Triggered prospectives (`check_triggers` results) are unaffected.
- Recalled procedures (`recall_procedure` results) are unaffected
  by PR-A. PR-C adds new behaviour to that surface.

---

## Observability

A single `tracing::info!` line is emitted per preparation pass with
low-cardinality counters:

```
[simard] preparation: 0 snapshot facts, 3 stale goal-store filtered, 9 facts retained
```

Fields:

| Field                              | Meaning                                              |
|------------------------------------|------------------------------------------------------|
| `snapshot facts`                   | Count dropped by filter 1                            |
| `stale goal-store filtered`        | Count dropped by filter 2 (after slug-dedup)         |
| `facts retained`                   | Final length of `PreparedContext.relevant_facts`     |

Grep this line in daemon logs to confirm the filters fired and to
spot regressions where redundant facts re-enter the prepared context.

---

## Where the snapshot concept constant lives

The new `GOAL_BOARD_SNAPSHOT_CONCEPT` constant is defined in
`src/goals/cognitive_memory_store.rs` next to its sibling
`GOAL_STORE_FACT_CONCEPT`, and re-exported through `src/goals/mod.rs`
so the consolidation layer imports both from `crate::goals`:

```rust
// src/goals/cognitive_memory_store.rs
pub(crate) const GOAL_STORE_FACT_CONCEPT: &str = "goal-store:record";
pub(crate) const GOAL_BOARD_SNAPSHOT_CONCEPT: &str = "goal-board:snapshot";

// src/goals/mod.rs
pub(crate) use cognitive_memory_store::{
    GOAL_BOARD_SNAPSHOT_CONCEPT,
    GOAL_STORE_FACT_CONCEPT,
    GOAL_STORE_LIST_LIMIT,
};

// src/memory_consolidation/mod.rs
use crate::goals::{
    GOAL_BOARD_SNAPSHOT_CONCEPT,
    GOAL_STORE_FACT_CONCEPT,
    GOAL_STORE_LIST_LIMIT,
    GoalRecord,
};
```

Co-locating the two concept names with the writer
(`save_goal_board_snapshot` lives in `src/goal_curation/operations.rs`
but uses the constant from `crate::goals`) ensures that future
contributors searching for "snapshot concept" find the writer, the
reader, and the filter together.

---

## Fallback lever

If a downstream consumer (test harness, dashboard, future agent) is
discovered to depend on `goal-board:snapshot` facts appearing in
preparation output, filter 1 can be relaxed to "keep most recent
revision" with a one-line change. The relaxed form keeps the entry
with the lexicographically maximum `node_id` (which is monotonic
because node ids are timestamp-prefixed):

```rust
// keep only the newest revision per slug
let snapshots = raw_facts.iter()
    .filter(|f| f.concept == GOAL_BOARD_SNAPSHOT_CONCEPT)
    .max_by_key(|f| f.node_id.clone());
```

PR-A ships the strict drop-all variant because no current consumer
depends on snapshots in preparation. The lever is documented here so
future contributors do not have to re-derive it.

---

## Examples

### Example 1 — typical cycle

Before PR-A:

```
preparation_summary: 16 facts
  - 10× goal-board:snapshot (identical JSON blobs from cycles 41–50)
  - 4×  goal-store:record   (active: fix-auth, refactor-mod;
                             stale:  adopt-tdd, decision-use-rust)
  - 2×  bug-pattern         (genuinely useful)
```

After PR-A:

```
preparation_summary: 4 facts
  - 2× goal-store:record    (active: fix-auth, refactor-mod)
  - 2× bug-pattern          (genuinely useful)
```

Log line:

```
[simard] preparation: 10 snapshot facts, 2 stale goal-store filtered, 4 facts retained
```

### Example 2 — only stale goal-store records

Input: 5 `goal-store:record` facts for slugs `A B C D E`;
`active_slugs = {A, B}`.

After preparation: 2 facts (A and B). Log line:

```
[simard] preparation: 0 snapshot facts, 3 stale goal-store filtered, 2 facts retained
```

### Example 3 — no filters fire

Input: 6 facts of mixed concepts, none `goal-board:snapshot`, all
`goal-store:record` slugs on active list.

After preparation: 6 facts. Log line:

```
[simard] preparation: 0 snapshot facts, 0 stale goal-store filtered, 6 facts retained
```

---

## Code location

| Item                                | File                                                   |
|-------------------------------------|--------------------------------------------------------|
| `preparation_memory_operations`     | `src/memory_consolidation/mod.rs`                      |
| `PreparedContext` struct            | `src/memory_consolidation/mod.rs`                      |
| Single call site                    | `src/ooda_loop/cycle.rs` (in `prepare_phase`)          |
| `GOAL_BOARD_SNAPSHOT_CONCEPT` const | `src/goals/mod.rs` (re-export of `src/goals/cognitive_memory_store.rs`, alongside `GOAL_STORE_FACT_CONCEPT`) |
| `GOAL_STORE_FACT_CONCEPT` const     | `src/goals/cognitive_memory_store.rs` (existing), re-exported via `src/goals/mod.rs` |
| Tests                               | `src/memory_consolidation/tests.rs`                    |

---

## Testing

Four unit tests live in `src/memory_consolidation/tests.rs`:

| Test                                                  | Coverage                                          |
|-------------------------------------------------------|---------------------------------------------------|
| `preparation_drops_goal_board_snapshot_revisions`     | Filter 1: 5 snapshot revisions → 0 in output      |
| `preparation_drops_stale_goal_store_records`          | Filter 2: stale slugs dropped                     |
| `preparation_keeps_active_goal_store_records`         | Filter 2 regression: active slugs not dropped     |
| `preparation_does_not_filter_other_concepts`          | Random concepts untouched by either filter        |

Each test injects a fixture `CognitiveMemoryOps` mock that returns
hand-built `Fact` lists and asserts the final `PreparedContext`
contents.

---

## Out of scope

These were considered for PR-A and explicitly deferred:

- **`simard memory prune-stale` CLI command** — would let an operator
  delete stale `goal-store:record` rows from storage rather than
  filtering them at read time. PR-A's runtime filter resolves the
  noise problem stated in the acceptance criteria; the storage GC is
  an independent concern with its own design tradeoffs (replay-safety,
  audit trail, recovery semantics) and will land as a follow-up if
  storage growth becomes a problem.
- **Snapshot eviction from storage** — the snapshot facts continue to
  accumulate in semantic memory. PR-A only filters at preparation
  time. A separate retention policy is the right home for storage
  eviction.
- **Per-concept fact budgets** — the existing 16-fact budget remains
  flat across all concepts. A future PR could allocate budget
  per-concept to guarantee diversity, but the two PR-A filters alone
  resolve the duplication that motivated the work.
