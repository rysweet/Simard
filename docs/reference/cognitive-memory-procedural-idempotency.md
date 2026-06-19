---
title: Procedural-memory store idempotency
description: How store_procedure deduplicates on exact procedure name so repeated OODA consolidation cycles stop re-storing identical procedures, ending the 0% compression / frozen procedural-store defect.
last_updated: 2026-06-19
owner: simard
doc_type: reference
related:
  - ./ooda-procedural-memory.md
  - ./cognitive-memory-bootstrap-procedures.md
  - ../architecture/episode-distillation.md
  - ../architecture/cognitive-memory.md
  - ../memory.md
---

# Procedural-memory store idempotency

> Shipped in issue [#2298](https://github.com/rysweet/Simard/issues/2298).
> Supersedes the "one procedure node per successful cycle / intentional
> accumulation" behaviour described in
> [OODA procedural memory](./ooda-procedural-memory.md) and
> [Bootstrap procedures and trigger naming](./cognitive-memory-bootstrap-procedures.md)
> for the case of **identically-named** procedures.

`store_procedure` is **idempotent on the exact procedure name**. Storing a
procedure whose `name` already exists is a no-op for node creation: no new
`Procedure` node is written, and the existing node id is returned. Genuinely
new procedures (distinct names) still accumulate exactly as before.

This closes the non-idempotency defect in which every OODA consolidation
cycle re-stored the same handful of procedures
(`consolidate:ad-hoc`, `pr-merge:adopt-tdd`, `pr-merge:fix-broken-features`),
producing 0% procedural compression and a procedural store that only ever
recalled the 5 bootstrap procedures even as the node table grew unbounded.

---

## The defect (before #2298)

Procedural learning runs in the OODA Act phase: every **successful**
`ActionOutcome` is stored as a procedure via `store_procedure`
(`src/ooda_loop/cycle.rs`). The procedure name is derived from the action
pattern, goal scope, and triggers by `compose_procedure_name`, e.g.:

```
consolidate:ad-hoc | triggers: consolidate,memory,distill,g
```

Because the same actions (`ConsolidateMemory`, the two seeded `pr-merge`
patterns, …) succeed on most cycles, `compose_procedure_name` produces the
**same name** cycle after cycle. The pre-#2298 `store_procedure`
implementation issued an unconditional `CREATE` with a fresh
`new_id("proc")` and had no dedup on `name`; the `Procedure` table keys on
`id`, not `name`, so the database did not dedup either:

```cypher
-- pre-#2298: one new node every call, regardless of name collision
CREATE (p:Procedure {id: 'proc_<fresh-uuid>', name: '<name>', ...})
```

Symptoms operators saw:

- The daemon logged the same line every consolidation cycle:
  ```
  [simard] OODA consolidation: stored procedure 'consolidate:ad-hoc | triggers: consolidate,memory,distill,g'
  ```
- `get_statistics().procedural_count` grew every cycle, but
  `recall_procedure` only ever surfaced the 5 bootstrap procedures (the
  duplicates were the same names ranked the same way).
- Procedural compression sat at **0%** — no learned procedures ever
  *accumulated* in a meaningful, queryable way; the store was effectively
  frozen.

### What was NOT the cause

Two of the three originally-suspected candidates were investigated and
ruled out — they are correct and are left untouched:

| Candidate | Verdict |
|-----------|---------|
| `mark_episode_distilled` not persisting | **Not the cause.** `SET e.distilled = 1` + `post_write_barrier` persist correctly and idempotently (`ops.rs`). |
| `list_undistilled_episodes` re-returning the same episodes | **Not the cause.** The `WHERE e.distilled = 0` gate excludes marked rows (`ops.rs`). |
| Procedural-store upsert creating new nodes instead of updating | **Confirmed root cause.** `store_procedure` had no name dedup. |

Note that **episode distillation** (which writes *facts*, not procedures —
see [Episode distillation](../architecture/episode-distillation.md)) was
already idempotent via the `distilled` marker. The frozen-procedural symptom
came entirely from the OODA procedural-learning write path, not from the
distillation pass.

---

## Behaviour (after #2298)

### Dedup key: exact `name` equality

The deduplication key is the **full procedure name string**, compared for
exact equality. The name already encodes the procedure's identity:

```
{pattern}:{scope} | triggers: {comma-separated-keywords}
```

(See [Bootstrap procedures and trigger naming](./cognitive-memory-bootstrap-procedures.md)
for how the name is composed.) Two stores with identical pattern, scope, and
trigger list therefore produce identical names and dedup to a single node.
Two stores that differ in any of those fields produce distinct names and
remain distinct nodes — learning still accumulates.

Exact equality (not the `CONTAINS` substring match used by
`recall_procedure`) is used deliberately: a substring/`CONTAINS` probe could
falsely match an unrelated procedure that merely shares trigger tokens, which
would suppress a genuinely new procedure. This mirrors the exact-name filter
the bootstrap seeder already applies (`seed_bootstrap_procedures`).

### `store_procedure` is a read-before-write upsert

```rust
fn store_procedure(
    &self,
    name: &str,
    steps: &[String],
    prerequisites: &[String],
) -> SimardResult<String>;
```

Contract:

1. **Existing name** → no `CREATE`. The existing node's id is returned, that
   node's `usage_count` is incremented (`SET p.usage_count = p.usage_count + 1`),
   and the standard `post_write_barrier` still runs. Calling again with the
   same name returns the **same id**. This path is therefore *not* a pure
   no-op: it is idempotent on the **node count** while deliberately recording
   the recurrence (see *Reinforcement signal: `usage_count`* below).
2. **New name** → a `Procedure` node is created exactly as before
   (`CREATE … usage_count: 0`, followed by `post_write_barrier`), and the
   new id is returned.
3. The method remains **total and idempotent on node count**: any number of
   calls with the same `name` leave the `Procedure` node count unchanged
   after the first (only `usage_count` advances).

The existence check is a single indexed lookup using the same
`escape_cypher` escaping as the create path:

```cypher
MATCH (p:Procedure) WHERE p.name = '<escaped-name>' RETURN p.id LIMIT 1
```

Because idempotency lives **inside** `store_procedure`, every caller inherits
it — the OODA cycle, the bootstrap seeder, and any future writer — without
each caller needing its own recall-then-store guard.

### Reinforcement signal: `usage_count`

On the existing-name path, `store_procedure` bumps the stored procedure's
`usage_count` (`SET p.usage_count = p.usage_count + 1`) so that a recurring
procedure still records its reinforcement/recurrence for future ranking by
`recall_procedure`. This is a counter update on a single existing row, **not**
a new node. Idempotency is defined over the *node count*, not over
`usage_count`; regression tests assert node-count stability and never assert
a frozen `usage_count`.

### Trait surface is unchanged

The `CognitiveMemoryOps::store_procedure` signature
(`-> SimardResult<String>`) is **unchanged**. The fix is internal to
`NativeCognitiveMemory`, so none of the other implementations
(`CognitiveMemoryBridge`, `RemoteCognitiveMemory`, `SharedMemory`, and the
test/meeting stubs) change shape, and no IPC/wire-protocol change is needed
to convey a "created vs. existing" flag across process boundaries.

---

## OODA log honesty

The OODA consolidation call site (`src/ooda_loop/cycle.rs`) emits the
`stored procedure` line **only when a procedure is actually created**, not on
every cycle. It performs the same exact-name pre-check the seeder uses
(`recall_procedure(name, N)` filtered to `h.name == name`) and logs only on
the create path:

```
# first time a given name is learned:
[simard] OODA consolidation: stored procedure 'consolidate:ad-hoc | triggers: consolidate,memory,distill,g'

# subsequent cycles producing the same name: no "stored procedure" line
```

The underlying `store_procedure` call is left in place on both paths and
stays safe either way — the pre-check governs the log line, while
`store_procedure`'s internal dedup governs correctness. A failure to store is
still logged on the existing best-effort error branch:

```
[simard] OODA consolidation: procedural memory failed: <error>
```

### Cost: a second existence lookup on the OODA path

Keeping the trait signature unchanged (no "created vs. existing" return flag)
means the create-vs-existing decision is discovered by *querying*, not by
reading a value the caller already holds. Two lookups therefore run per
successful outcome on the OODA path:

1. the log-guard pre-check in `cycle.rs`
   (`recall_procedure(name, N)` filtered to `h.name == name`), which decides
   whether to emit the `stored procedure` log line, and
2. the existence `MATCH` **inside** `store_procedure`, which decides whether
   to `CREATE`.

This duplication is the deliberate, accepted cost of leaving the trait shape
(and the IPC/wire surface) untouched. It is bounded — two indexed single-row
lookups per successful action, on a consolidation path that already performs
network/LLM work each cycle, so it is not a hot loop. A future optimisation
could collapse the two by having `store_procedure` return a `created: bool`,
but that changes the trait and ripples through all seven implementations,
which #2298 explicitly avoids.

---

## Effect on compression

Procedural "compression" is an **observable consequence** of dedup, not a
separately-computed metric. Before #2298, repeated identical stores meant the
ratio of distinct-recallable procedures to total stores stayed at ~0%. After
#2298, identical stores collapse to one node, so the procedural store
reflects only genuinely distinct learned procedures and recall returns an
accumulating, de-duplicated set. There is no hardcoded `0%` path to remove —
the number simply stops being pinned to zero once duplicates no longer land.

---

## Examples

### Example 1 — repeated consolidation cycle (the fixed case)

```
Cycle N:
  ConsolidateMemory succeeds
  compose_procedure_name → "consolidate:ad-hoc | triggers: consolidate,memory,distill,g"
  store_procedure(name, steps, &[])
    → name absent → CREATE node proc_abc, usage_count = 0
  log: [simard] OODA consolidation: stored procedure 'consolidate:ad-hoc | ...'

Cycle N+1 (same objective shape):
  ConsolidateMemory succeeds
  compose_procedure_name → identical name
  store_procedure(name, steps, &[])
    → name present → NO new node, returns proc_abc, usage_count = 1
  (no "stored procedure" log line)

get_statistics().procedural_count: unchanged across N → N+1
recall_procedure("consolidate memory", 5): one consolidate:ad-hoc entry
```

### Example 2 — genuinely new procedure still accumulates

```
store_procedure("pr-merge:fix-auth | triggers: merge,pr,2310,rs", steps, &[])
  → new name → CREATE → procedural_count increments
store_procedure("pr-merge:fix-cache | triggers: merge,pr,2311,rs", steps, &[])
  → different name → CREATE → procedural_count increments again
```

Distinct learning is preserved; only exact-name repeats are deduped.

### Example 3 — bootstrap seeding stays idempotent

`seed_bootstrap_procedures` already filtered by exact name before storing, so
its count semantics are unchanged. With the in-`store_procedure` guard now
present, the seeder is idempotent even if its own pre-filter were removed —
restarting the daemon never duplicates the 5 bootstrap procedures.

---

## Code location

| Item | File |
|------|------|
| `store_procedure` (idempotent upsert) | `src/cognitive_memory/ops.rs` (`NativeCognitiveMemory`) |
| `store_procedure` trait method (signature unchanged) | `src/cognitive_memory/mod.rs` (`CognitiveMemoryOps`) |
| OODA call site + log guard | `src/ooda_loop/cycle.rs` (procedural-learning loop) |
| `compose_procedure_name` (name derivation, unchanged) | `src/ooda_loop/cycle.rs` |
| Bootstrap seeder (exact-name precedent) | `src/cognitive_memory/bootstrap_procedures.rs` |
| `mark_episode_distilled` / `list_undistilled_episodes` (unchanged) | `src/cognitive_memory/ops.rs` |

---

## Testing

The fix is delivered test-first (TDD). The regression test is written and
**fails on the pre-#2298 code** (the duplication assertion), then passes once
the dedup lands.

### Regression test (store layer)

Against `NativeCognitiveMemory::in_memory()` (the `test_mem()` helper in
`src/cognitive_memory/tests_pr_2298_idempotency.rs`) — **not** the
`EpisodeMock`/`ProcMock` stubs, whose `store_procedure` is a no-op that would
hide the bug:

| Test | Coverage |
|------|----------|
| `store_procedure_is_idempotent_on_exact_name` | Store the same name twice → wildcard recall filtered to that name has count **1** (pre-fix: 2 → fails). |
| `store_procedure_returns_stable_id_for_duplicate` | Second store of an identical name returns the **same id** as the first (pre-fix: a fresh `proc_…` id → fails). |
| `store_procedure_preserves_distinct_named_procedures` | Two different names → two distinct nodes (guards against over-dedup). |
| `repeated_ooda_consolidation_does_not_re_store_procedures` | Faithful symptom reproduction: the three daemon-log procedures stored on two cycles → one node each (pre-fix: two each → fails). |
| `store_procedure_reinforces_usage_count_on_duplicate` | Duplicate store leaves node count stable but increments `usage_count` (reinforcement signal). |

> **`"*"` recall caveat.** `recall_procedure("*", N)` applies a hard `LIMIT N`
> with **no ordering** — it returns rows in store order, capped at `N`. Any
> count assertion built on it must pass an `N` strictly greater than the total
> number of procedures in the test store, or rows are silently truncated and a
> real duplicate could be missed. The tests size `N` accordingly (e.g. recall
> with a generous limit, or filter `recall_procedure(name, N)` to the specific
> name and assert the filtered count) rather than trusting a small fixed `N`.

### Combined-invariant test

`consolidation_cycle_is_idempotent_and_keeps_episodes_distilled` asserts both
halves of the issue in one pass: after a deterministic
distillation/consolidation pass over a fixed episode set,

1. processed episodes are absent from `list_undistilled_episodes(N)` (the
   already-correct distillation marker, guarded against regression), **and**
2. a second identical procedural pass leaves the `Procedure` node count
   unchanged.

### Existing suites that must stay green

- `src/cognitive_memory/bootstrap_procedures_tests.rs` — seeding stays
  idempotent (`seed_is_idempotent`, `seed_skips_existing_procedures_by_name`).
- `src/cognitive_memory/ops.rs` round-trip tests
  (`store_procedure_returns_prefixed_id`,
  `recall_procedure_returns_steps_and_prerequisites`).
- Hermetic-guard tests (`tests_hermetic_parity.rs`, the
  `assert_hermetic_for` guard at the top of `store_procedure`).
- `cargo test` for the `cognitive_memory`, `memory_consolidation`, and
  `ooda_loop` modules.

---

## Migration / backwards compatibility

- **Existing duplicate rows** written by pre-#2298 cycles remain in the
  database. They are still recalled; they are not retroactively merged.
  `store_procedure` simply stops *adding* new duplicates from this point on.
  A future `simard memory dedup-procedures` hygiene command could collapse
  the historical duplicates; #2298 does not delete data.
- **Trait shape** is unchanged — no method added, no signature changed, no
  IPC/wire-protocol change.
- **Tests** that previously relied on "each successful cycle adds a new
  procedure node" for an identical name are updated to assert node-count
  stability across repeats.

---

## Out of scope

- **Episodic recall and trigger derivation** (`search_episodes_by_keywords`,
  `compose_procedure_name`, `check_triggers`) — owned by separate
  workstreams; untouched here.
- **`mark_episode_distilled` / `list_undistilled_episodes`** — already
  correct; only regression-guarded.
- **A typed `triggers` column or a `UNIQUE(name)` schema constraint** — the
  read-before-write guard achieves idempotency without a schema migration; a
  DB-level uniqueness constraint is a possible future hardening.
- **A dedicated procedural-compression metric** — compression is treated as
  an observable outcome of dedup, not a new computed field.
- **Pruning historical duplicate rows** — left in place; a follow-up hygiene
  CLI can collapse them.

---

## Related reading

- [OODA procedural memory](./ooda-procedural-memory.md) — how successful
  OODA outcomes become procedures.
- [Bootstrap procedures and trigger naming](./cognitive-memory-bootstrap-procedures.md) —
  the naming convention that defines the dedup key and the exact-name seeder
  precedent.
- [Episode distillation](../architecture/episode-distillation.md) — the
  (already-idempotent) fact-distillation pass, distinct from procedural
  learning.
- [Cognitive Memory Architecture](../architecture/cognitive-memory.md) — the
  six memory types and their lifecycle.
- [Memory architecture (operator summary)](../memory.md).
