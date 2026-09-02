---
title: Cognitive-memory provenance (DERIVES_FROM edges)
description: How Simard records provenance when distilled facts and learned procedures are written to cognitive memory, linking each derived node back to the episode(s) it came from via DERIVES_FROM / PROCEDURE_DERIVES_FROM edges. Documents the CognitiveMemoryOps provenance API, the LibraryCognitiveMemory adapter behavior, the distillation and reflection wiring, and how to recall the source episodes of a fact.
last_updated: 2026-07-16
owner: simard
doc_type: reference
related:
  - ../memory.md
  - ../architecture/cognitive-memory.md
  - ../architecture/cognitive-memory-library-adapter.md
  - ../architecture/episode-distillation.md
  - ./cognitive-memory-fact-recall.md
  - ./ooda-procedural-memory.md
  - ./cognitive-memory-procedural-idempotency.md
---

# Cognitive-memory provenance (DERIVES_FROM edges)

> Shipped in issue [#2325](https://github.com/rysweet/Simard/issues/2325).
> Wires Simard's cognitive-memory **writes** to record provenance, so that
> distilled facts and learned procedures link back to the episodes they
> were derived from. This turns the previously flat node store into a
> connected graph.

Before #2325, every fact Simard distilled from an episode was stored as a
standalone node. The only trace of where a fact came from was a free-text
`source_id` such as `distill:ep-7f3a…` — a string, not a graph edge.
Nothing could be **traversed**: given a fact you could not ask "which
episodes produced this?", and given an episode you could not ask "what did
we learn from this?".

Provenance wiring closes that gap. When a fact or procedure is written
*with provenance*, Simard records a typed graph edge from the new node to
each source episode:

| Derived node | Edge type | Points at |
|---|---|---|
| Semantic **fact** | `DERIVES_FROM` | the episode(s) it was distilled from |
| **Procedure** | `PROCEDURE_DERIVES_FROM` | the episode(s) it was learned from |

These edges are created and stored by the upstream
[`amplihack-memory-lib`](https://github.com/rysweet/amplihack-memory-lib)
crate; Simard simply threads the source-episode ids through to the
library at write time and exposes a read method to traverse them back.

---

## What changed

| Aspect | Before (#2307 / PR-B) | Now (#2325) |
|---|---|---|
| Fact → episode link | textual `source_id` only (`distill:{id}`) | textual `source_id` **plus** a `DERIVES_FROM` graph edge |
| Procedure → episode link | none | optional `PROCEDURE_DERIVES_FROM` graph edges |
| `CognitiveMemoryOps` write API | `store_fact`, `store_procedure` | unchanged **plus** `store_fact_with_provenance`, `store_procedure_with_provenance` |
| `CognitiveMemoryOps` read API | `search_facts`, `recall_procedure`, … | unchanged **plus** `episodes_for_fact` |
| Library dependency pin | `7b81590…` | `758e0a7ac43b772855592a7a92e2867b1d8d886b` (`features = ["persistent"]`) |
| Distillation writer | `store_fact(…, source_id)` | `store_fact_with_provenance(…, &[source_episode_id])` |
| Reflection writer | `store_fact(…, "session:{id}")` | captures the transcript episode id and threads it as provenance |

The change is **additive and backward-compatible**:

- The legacy `store_fact` / `store_procedure` methods keep their exact
  signatures and behavior. Code that does not care about provenance is
  untouched.
- The textual `source_id` is **retained** alongside the new edge, so the
  `distill:` prefix convention used by
  [fact recall](./cognitive-memory-fact-recall.md) and grep-style
  filtering still works.
- There is **no migration or backfill**. Facts written before #2325
  simply carry zero `DERIVES_FROM` edges; `episodes_for_fact` returns an
  empty list for them, which is correct.

---

## API

The provenance surface lives on the `CognitiveMemoryOps` trait
(`src/cognitive_memory/mod.rs`). All three new methods are **defaulted**
on the trait so that **all seven** implementors keep compiling with no
edits. Only the live on-disk backend,
[`LibraryCognitiveMemory`](../architecture/cognitive-memory-library-adapter.md),
overrides them; the other six inherit the no-provenance defaults:

- `CognitiveMemoryClient` (`src/memory_client/mod.rs`) — in-process client,
- `RemoteCognitiveMemory` (`src/memory_ipc/client.rs`) — IPC client,
- `SharedMemory` (`src/memory_ipc/mod.rs`) — IPC server-side shared memory,
- and the three test stubs `ProcMock`, `MockClient`, `EpisodeMock`.

These are all native Rust implementors — the de-fork removed the Python
client, so there is no out-of-process RPC client to update.

### `store_fact_with_provenance`

```rust
fn store_fact_with_provenance(
    &self,
    concept: &str,
    content: &str,
    confidence: f64,
    source_id: &str,
    tags: Option<&[String]>,
    metadata: Option<&HashMap<String, serde_json::Value>>,
    source_episode_ids: &[String],
) -> SimardResult<String>;
```

Stores a semantic fact and, for each id in `source_episode_ids`, records
a `DERIVES_FROM` edge from the new fact node to that episode node.
Returns the new fact's `node_id`.

- `concept`, `content`, `confidence`, `source_id` — identical meaning to
  [`store_fact`](#legacy-methods-unchanged). The textual `source_id` is
  preserved; the graph edges are *additional* provenance, not a
  replacement.
- `tags` — optional concept/index tags (the library's `Option<&[String]>`
  tag slot).
- `metadata` — optional free-form metadata. The adapter stamps its own
  monotonic sequence key (`_simard_seq`, see
  [invariants](#adapter-invariants)) into this map before delegating, so
  callers may pass `None`.
- `source_episode_ids` — the episode `node_id`s this fact was derived
  from. Passing an **empty** slice stores the fact with no edges, i.e.
  behaves like `store_fact`.

> **Parameter-order note.** `store_fact_with_provenance` follows the
> *library's* argument order — `source_id` comes **before** `tags` — which
> is the opposite of the legacy `store_fact(concept, content, confidence,
> tags, source_id)`. Pass arguments in the order shown above.

**Default (non-library) impl.** Delegates to `store_fact`, dropping the
provenance edges and the extra metadata. Because `store_fact` takes
`tags: &[String]` (not an `Option`), the default clients the optional
tags slot with `tags.unwrap_or(&[])`. This keeps the six non-graph
backends (client / IPC / stubs) compiling and functionally correct — they
store the fact, just without edges.

### `store_procedure_with_provenance`

```rust
fn store_procedure_with_provenance(
    &self,
    name: &str,
    steps: &[String],
    prerequisites: &[String],
    source_episode_ids: &[String],
) -> SimardResult<String>;
```

Stores (or idempotently upserts) a procedure and records a
`PROCEDURE_DERIVES_FROM` edge to each id in `source_episode_ids`. Returns
the procedure's `node_id`.

- `name`, `steps`, `prerequisites` — identical meaning to
  [`store_procedure`](#legacy-methods-unchanged).
- The library backend preserves the **idempotent upsert-that-reinforces**
  contract documented in
  [Procedural-memory store idempotency](./cognitive-memory-procedural-idempotency.md):
  re-storing an identically-named procedure keeps the node count at one
  and bumps its `usage_count`. Provenance edges are added on top of that
  existing behavior.

**Default (non-library) impl.** Delegates to `store_procedure`, dropping
the provenance edges.

### `episodes_for_fact`

```rust
fn episodes_for_fact(&self, fact_id: &str) -> SimardResult<Vec<String>>;
```

The **recall** half of provenance. Returns the `node_id`s of every
episode the given fact `DERIVES_FROM`. This is what makes a provenance
link observable through Simard's own API rather than only inside the
library's graph.

- Returns an **empty** `Vec` when the fact has no provenance edges
  (including all pre-#2325 facts and facts stored with an empty
  `source_episode_ids`).
- The library backend implements this by traversing the fact's outgoing
  `DERIVES_FROM` neighbors in the `lbug` graph store. The traversal is a
  **single hop** (the fact's direct sources), not a transitive walk, so
  the result is bounded by the fact's out-degree — the number of episodes
  supplied at write time (one per distilled fact today). It cannot fan
  out across the wider graph.

**Default (non-library) impl.** Returns `Ok(vec![])`. Backends without a
graph layer report "no recorded provenance", which is accurate for them.

### Legacy methods (unchanged)

`store_fact`, `store_procedure`, `search_facts`, and `recall_procedure`
are **unchanged** in both signature and behavior. `store_fact` continues
to stamp `_simard_seq`, and `store_procedure` continues to perform the
idempotent reinforce. Existing callers need no changes.

---

## Adapter behavior

### Adapter invariants

The `LibraryCognitiveMemory` overrides preserve every invariant the
legacy write paths uphold:

- **Monotonic fact sequence.** `store_fact_with_provenance` stamps the
  process-wide `_simard_seq` key into `metadata` exactly as `store_fact`
  does, so `to_fact` can continue to expose a time-ordered `node_id`. The
  sequence is fetched while the write lock is held so order matches store
  order.
- **Single exclusive lock (no `RwLock`).** The inner library memory is a
  `Mutex<CognitiveMemory>`, so every access is exclusive. Reads acquire it
  via `lock()`; writes via `lock_write()`, which is the *same* exclusive
  acquisition plus a `cfg(test)`-only hermetic-state-root guard (compiled
  out of release builds). There is no shared/read lock. The fact (with its
  `_simard_seq` stamp) and its `DERIVES_FROM` edges are written under one
  `lock_write()` hold, so a concurrent reader can never observe a fact
  mid-link; `episodes_for_fact` takes the same mutex via `lock()`.
- **Crash atomicity.** The adapter delegates to the library's combined
  `store_fact_with_provenance` (fact + edges in a single library call), so
  the fact and its edges land as one persistent-store operation — there is
  no fact-without-edges window. If the pinned library rev instead requires
  composing from primitives (`store_fact` then `link_fact_to_episodes`),
  the single lock still prevents partial *reads*, but a process crash
  *between* those two library writes would leave the fact with zero edges.
  Edges are always written **after** the fact, so the only possible
  degraded state is "fact present, no provenance" — never a dangling edge
  — and `episodes_for_fact` already returns `[]` for exactly that case.
  Provenance therefore degrades gracefully and is never observed
  half-written.
- **Idempotent procedure upsert.** `store_procedure_with_provenance`
  keeps the exact-name dedup + `usage_count` reinforcement of
  `store_procedure`, then records `PROCEDURE_DERIVES_FROM` edges.
- **Fail-closed locking.** A poisoned lock surfaces as a storage error
  (`StoragePoisoned`); provenance writes never silently degrade.

### Recall returns ids, not content

`episodes_for_fact` returns episode **ids** only. To materialize episode
content, follow up with an episode read (for example via the
episodic-recall paths described in
[Episodic keyword recall](./cognitive-memory-episodic-recall.md)). Keeping
recall id-only avoids loading episode bodies into error/trace paths.

---

## Where provenance is recorded

Two production write paths thread provenance automatically. Application
code does not need to change to benefit.

### Distillation (facts)

[Episode distillation](../architecture/episode-distillation.md) already
knows each fact's originating episode — every `DistilledFact` carries a
`source_episode_id`. The distillation writer now calls
`store_fact_with_provenance`, passing that id as a one-element
`source_episode_ids` slice:

```
Recipe output: { "facts": [ { concept, content, source_episode_id }, … ] }
  │
  ▼
For each fact:
    store_fact_with_provenance(
        concept,
        content,
        confidence = 0.7,
        source_id  = format!("distill:{source_episode_id}"),   // retained
        tags       = Some(&[concept]),
        metadata   = None,                                     // adapter stamps _simard_seq
        source_episode_ids = &[source_episode_id],             // ← DERIVES_FROM edge
    )
  │
  ▼
A DERIVES_FROM edge now links the new fact → its source episode.
```

The textual `distill:{id}` `source_id` is kept for backward compatibility;
the edge is the new, traversable provenance.

> There is currently no production path that *distills procedures* from
> episodes, so `store_procedure_with_provenance` has no distillation call
> site today. The method exists so that any future procedure-distillation
> path records provenance the same way. The OODA Act phase, which is the
> sole procedure writer, may adopt it once it tracks the originating
> episode.

### Reflection (facts)

In the reflection phase
(`memory_consolidation::reflection_memory_operations`), the session
transcript is stored as an episode and any extracted facts are written to
semantic memory. The transcript episode id returned by `store_episode` is
now captured and threaded as provenance into each derived fact:

```rust
let episode_id = client.store_episode(
    &format!("Session {session_id} transcript: {transcript}"),
    "session-reflection",
    None,
)?;

// … for each extracted, deduplicated fact …
client.store_fact_with_provenance(
    &fact.concept,
    &fact.content,
    fact.confidence,
    &format!("session:{session_id}"),   // textual source_id retained
    None,                               // tags
    None,                               // metadata (adapter stamps _simard_seq)
    &[episode_id.clone()],              // ← DERIVES_FROM edge to the transcript episode
)?;
```

The cross-session dedup and the textual `session:{id}` `source_id` are
unchanged; the only addition is the `DERIVES_FROM` edge to the transcript
episode.

---

## Usage

### Recording provenance explicitly

```rust
use std::collections::HashMap;
use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

let mem = LibraryCognitiveMemory::open(&state_root)?;

// Store an episode and capture its id.
let episode_id = mem.store_episode(
    "CI failed on a linker OOM; raised RUSTFLAGS=-Clink-arg=-Wl,--no-keep-memory",
    "engineer-cycle",
    None,
)?;

// Distill a fact from it, recording provenance.
let fact_id = mem.store_fact_with_provenance(
    "bug-pattern",
    "Linker OOM on CI is mitigated by --no-keep-memory",
    0.7,
    &format!("distill:{episode_id}"),  // textual source_id (retained)
    Some(&["bug-pattern".to_string()]),
    None,
    &[episode_id.clone()],             // DERIVES_FROM edge
)?;
```

### Recalling the source episodes of a fact

```rust
let sources = mem.episodes_for_fact(&fact_id)?;
assert!(sources.contains(&episode_id));
// `sources` is empty for legacy facts or facts stored without provenance.
```

### Procedures

```rust
let proc_id = mem.store_procedure_with_provenance(
    "ci-fix:linker-oom",
    &["set RUSTFLAGS link-arg".into(), "re-run the failing job".into()],
    &["CI job failed with OOM in ld".into()],
    &[episode_id],   // PROCEDURE_DERIVES_FROM edge
)?;
```

Re-storing the same procedure name is an idempotent reinforce (the node
count stays at one); see
[Procedural-memory store idempotency](./cognitive-memory-procedural-idempotency.md).

---

## Configuration

There is nothing to enable or configure. Provenance recording is **always
on** wherever a write site supplies source-episode ids:

- The library dependency is pinned to rev
  `758e0a7ac43b772855592a7a92e2867b1d8d886b` with `features =
  ["persistent"]` in `Cargo.toml`, which is the rev that exposes the
  provenance API.
- The persistent `lbug` `GraphStore` at `state_root/cognitive` stores the
  `DERIVES_FROM` / `PROCEDURE_DERIVES_FROM` edges alongside the existing
  six memory types. No new on-disk location or store is introduced.
- Backends other than `LibraryCognitiveMemory` transparently record no
  edges (the trait defaults), so nothing breaks if a non-graph backend is
  ever wired in.

---

## Backward compatibility

- **No data migration.** Facts and procedures written before #2325 keep
  working exactly as before; they simply have no provenance edges.
  `episodes_for_fact` returns an empty list for them.
- **No behavior change for legacy writers.** `store_fact` and
  `store_procedure` are byte-for-byte unchanged. A caller that has not
  adopted the provenance methods sees identical behavior.
- **Textual `source_id` preserved.** The `distill:` and `session:`
  prefixes that downstream tooling greps on are still written; the graph
  edge is purely additive.

---

## Observability: grounding-coverage self-metric

Recording provenance edges is only half the story — you also want to *see*
whether facts are actually being grounded over time. Two layers surface that:

| Layer | Series | Cadence | Shape |
|---|---|---|---|
| OpenTelemetry gauges | `simard.memory.edges{type=DERIVES_FROM…}` | per cycle | raw edge **counts** |
| Durable self-metric | **`fact_provenance_coverage`** | per cycle | grounded **fraction** `[0.0, 1.0]` |

The durable `fact_provenance_coverage` self-metric is the graph-memory *health*
signal. Once per OODA cycle the daemon reads the
[`graph_stats()`](#code-entry-points) snapshot it already collects for the OTel
edge gauges and emits one sample to the `metrics.jsonl` series:

```
fact_provenance_coverage = facts_with_provenance / facts_total
```

- **What it measures.** The fraction of semantic facts connected into the
  `DERIVES_FROM` provenance graph. The denominator is every semantic node the
  store reports (live plus archived/superseded revisions, matching
  [`GraphStats`](#code-entry-points)), so the series tracks grounding across the
  whole semantic layer — not just the newest facts.
- **Why a ratio, not the raw counts.** The OTel `simard.memory.edges` gauges
  already carry the raw counts, but they are point-in-time telemetry, not the
  durable, comparable, regressable `metrics.jsonl` series that feeds gym-history
  regression signals. A *ratio* is store-size-independent, so a grounding
  regression (facts entering semantic memory without a provenance edge — which
  also loses the dominant term in the [reliability gate](./trustworthy-confidence-api.md))
  raises the same signal every other cognition self-metric
  (`recall_precision_at_k`, `distill_fact_yield`, `controlled_forgetting`) does.
- **Undefined on an empty store.** When the store holds zero facts, coverage is
  *undefined* and **no** sample is emitted (skip rather than drag the series to a
  misleading `0.0`), mirroring the `recall_precision_at_k` convention. The
  emitter is best-effort — a metrics-write failure is logged, never propagated —
  and pure observation: it never changes memory state.

The scoring is a pure function
(`cognitive_memory::metrics::provenance_coverage`) with the per-cycle emitter
(`record_provenance_coverage_metric`) beside the existing
`flush_recall_precision_metric`, so both live next to the recall-quality
self-metrics they sit alongside.

---

## Observability: snapshot-dedup-hygiene self-metric

Grounding coverage watches whether facts enter the graph *connected*; a sibling
self-metric watches whether the **goal-board snapshot layer stays lean**.
Goal-board snapshots are revisioned: each new revision `SUPERSEDES` the prior
one, and
`prune_superseded` (controlled forgetting) reclaims the archived revisions over
time. `graph_stats()` already reports two raw counts for this layer:

| Field | Meaning |
|---|---|
| `snapshot_facts_total` | every goal-board snapshot revision still held (live + not-yet-pruned superseded) |
| `distinct_snapshot_caller_keys` | distinct logical goal-board snapshot streams behind them |

The durable `goal_board_snapshot_dedup_ratio` self-metric is the hygiene *health*
signal derived from them. After each successful OODA cycle, when `graph_stats()`
succeeds, the daemon emits one sample to the `metrics.jsonl` series from the
**same** snapshot it already collects for the OTel edge gauges and the
grounding-coverage metric:

```
goal_board_snapshot_dedup_ratio = distinct_snapshot_caller_keys / snapshot_facts_total
```

- **What it measures.** The average *liveness* of goal-board snapshot streams,
  in `[0.0, 1.0]`. `1.0` means every stream holds a single live revision; the
  value falls toward `0` as superseded revisions pile up. When every snapshot
  fact has a valid caller key, its inverse — total / distinct — is the mean
  revisions retained per stream.
- **Why it matters.** That accumulation is exactly the monotonic-growth failure
  controlled forgetting exists to prevent: if `prune_superseded` stops keeping
  pace, archived revisions bloat semantic memory. Previously that was visible
  only as the raw `graph_stats()` counts. The store-size-independent ratio adds
  a durable, comparable history for operator analysis and future automated
  regression detection; it does not currently create a Gym history signal.
- **Undefined on an empty goal-board snapshot layer.** When the store holds zero
  goal-board snapshot facts, the ratio is *undefined* and **no** sample is
  emitted (skip rather than
  drag the series to a misleading `0.0`), mirroring the `fact_provenance_coverage`
  convention. `distinct_snapshot_caller_keys` is clamped to `snapshot_facts_total`
  defensively so a miscount can never yield a ratio above `1.0`. The emitter is
  best-effort — a metrics-write failure is logged, never propagated — and pure
  observation: it never changes memory state.

The scoring is a pure function
(`cognitive_memory::metrics::goal_board_snapshot_dedup_ratio`) with the
per-cycle emitter (`record_goal_board_snapshot_dedup_ratio_metric`) beside
`record_provenance_coverage_metric`, so both graph-memory hygiene self-metrics
sit together.

---

## Testing

Provenance is covered by a TDD round-trip test (in
`src/cognitive_memory/tests_provenance.rs`) that:

1. opens an in-memory backend via `LibraryCognitiveMemory::in_memory()`
   (whose store implements the neighbor traversal; a tempdir `persistent`
   store is the fallback if the in-memory store ever lacks edge support),
2. stores an episode,
3. distills it into a fact via `store_fact_with_provenance` (the same path
   the distillation writer uses), passing the episode as the source, and
4. asserts that `episodes_for_fact(fact_id)` contains the episode id.

The test fails against the pre-#2325 code (the default `episodes_for_fact`
returns an empty list) and passes once the adapter records and traverses
the `DERIVES_FROM` edge — proving the link is recallable end-to-end
through Simard's own API.

Goal-board snapshot hygiene is covered by:

1. ratio boundary tests in `src/cognitive_memory/metrics.rs`,
2. a hermetic injected-writer metric-entry construction test that asserts the
   metric name, value, and serialized context (it does not exercise the real
   JSONL storage path), and
3. a daemon wiring test that passes asymmetric `GraphStats` counts through
   `record_graph_memory_self_metrics` and the goal-board emitter's injected
   writer, then asserts the resulting metric name, `0.25` value, and serialized
   `snapshot_facts` / `distinct_caller_keys` context. The asymmetric counts make
   the test fail if the numerator and denominator are reversed.

The process-boundary `memory stats --json` test in
`tests/bin_simard_memory_cli.rs` separately proves the same goal-board counts
are exposed to operators.

Run the relevant suites with:

```bash
cargo test cognitive_memory
cargo test memory_consolidation
cargo test graph_memory_metric_sweep_uses_goal_board_graph_stats_fields
cargo test --test bin_simard_memory_cli stats_shows_edges_and_dedup_section_via_direct_open
```

---

## Code entry points

- `src/cognitive_memory/mod.rs` — `CognitiveMemoryOps` trait: the three
  defaulted provenance methods (`store_fact_with_provenance`,
  `store_procedure_with_provenance`, `episodes_for_fact`).
- `src/cognitive_memory/library_adapter.rs` — `LibraryCognitiveMemory`
  overrides that delegate to the library's provenance API and traverse
  `DERIVES_FROM` for recall; `graph_stats()` computes the
  `facts_with_provenance` / `facts_total` snapshot the coverage metric reads.
- `src/cognitive_memory/metrics.rs` — `provenance_coverage()` (pure ratio) and
  `record_provenance_coverage_metric()` (per-cycle `fact_provenance_coverage`
  emitter), plus the sibling `goal_board_snapshot_dedup_ratio()` /
  `record_goal_board_snapshot_dedup_ratio_metric()` (per-cycle
  `goal_board_snapshot_dedup_ratio` emitter), plus the `GraphStats` snapshot
  type in `src/memory_cognitive.rs`.
- `src/operator_commands_ooda/daemon/mod.rs` — the per-cycle sweep that reads
  `graph_stats()` for the OTel edge gauges and emits both graph-memory
  self-metrics from the same snapshot.
- `src/memory_consolidation/distillation.rs` — distillation writer that
  threads `source_episode_id` as `DERIVES_FROM` provenance.
- `src/memory_consolidation/mod.rs` — `reflection_memory_operations` that
  threads the transcript episode id as provenance.
- `src/cognitive_memory/tests_provenance.rs` — the recallable-provenance
  TDD test.

---

## Related

- [Memory architecture](../memory.md) — the six-type model and
  consolidation flow.
- [Cognitive Memory Architecture](../architecture/cognitive-memory.md) —
  canonical schema and consolidation rules.
- [Library-backed Cognitive Memory](../architecture/cognitive-memory-library-adapter.md) —
  the `amplihack-memory-lib` backend that owns the graph edges.
- [Episode distillation](../architecture/episode-distillation.md) — the
  pass that produces facts (and now their provenance).
- [Cognitive-memory fact recall](./cognitive-memory-fact-recall.md) — how
  facts (including the `distill:`-tagged ones) surface during preparation.
- [OODA procedural memory](./ooda-procedural-memory.md) — the procedure
  writer that may adopt provenance.
- [Procedural-memory store idempotency](./cognitive-memory-procedural-idempotency.md) —
  the upsert-that-reinforces contract preserved by
  `store_procedure_with_provenance`.
