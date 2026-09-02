---
title: Bulk graph-adjacency index for ranked recall (prepare-context performance)
description: Planned design — how ranked declarative and episodic recall will score the graph-proximity signal from a single per-recall bulk adjacency load instead of ~3N per-node neighbour round-trips, targeting a cut in OODA prepare-context wall-clock from ~11 minutes toward single-digit seconds at ~7,590 facts while preserving byte-identical ranking, per-agent tenant isolation, and tombstone exclusion.
last_updated: 2026-07-08
owner: simard
doc_type: reference
related:
  - ../memory.md
  - ../architecture/cognitive-memory.md
  - ../architecture/cognitive-memory-library-adapter.md
  - ./cognitive-memory-ranked-recall.md
  - ./cognitive-memory-ranked-episodic-recall.md
  - ./cognitive-memory-provenance.md
---

# Bulk graph-adjacency index for ranked recall

> **Status: Shipped — issue
> [#40](https://github.com/rysweet/Simard/issues/40)**
> (`perf(cog-mem): bulk graph-adjacency index — prepare-context from ~11 min to
> seconds`). The engine mechanism shipped in
> [`amplihack-memory-lib` PR #126](https://github.com/rysweet/amplihack-memory-lib/pull/126)
> and Simard consumes it via the pinned `amplihack-memory` git rev
> (`72c5ea1`). It builds
> directly on
> [Phase-weighted ranked fact recall](./cognitive-memory-ranked-recall.md) and
> [Ranked episodic recall](./cognitive-memory-ranked-episodic-recall.md), whose
> **graph** scoring signal this page makes fast. Delivery order: the engine PR
> merged first, then the Simard pin bump; the persistent daemon now runs the
> indexed path described below (`graph_path = indexed`), with the legacy per-node
> path under [Configuration](#configuration) retained only for backends that do
> not implement the bulk capability.

The OODA daemon's per-cycle **prepare-context** step
(`preparation_memory_operations_with_active_slugs_phased`, in
`src/memory_consolidation/mod.rs`) spins for **~11 minutes at ~300 % CPU
every cycle** with no child processes and no log output — the substance behind
the operator complaint *"the memory graph never loads."* The graph does load; the
read is pathologically slow, so each OODA cycle takes ~15–18 min and the
dashboard / memory views feel stuck.

Root cause is **not** the size of the result (10 facts, 5 procedures, 5
episodes) but the cost of computing the **graph-proximity** term of ranked
recall. For every candidate node the ranker issues ~3 lock-serialized Cypher
neighbour scans over the entire semantic store — an **N+1 fan-out of ~23,000
queries per recall** at ~7,590 facts. Episodic recall repeats the pattern and
adds a full-store backfill.

The fix replaces that per-node fan-out with a **single bulk adjacency load per
recall**. Under the fix, each ranked recall performs a small, fixed number of
typed edge scans, builds an in-memory adjacency index once, and runs the
*identical* bounded breadth-first graph walk against that index. Ranking output
is byte-identical; only the wall-clock changes.

| Metric (per recall, ~7,590 semantic facts) | Before | After |
|---|---:|---:|
| Graph-proximity DB round-trips | ~23,000 | 2 (one per edge type) |
| Round-trip growth vs. node count `N` | `O(N)` | `O(1)` typed scans |
| `prepare-context` wall-clock (Observe) | ~11 min | single-digit s (target) |
| Ranking order / scores / reasons | baseline | **identical** |

---

## What the fix changes, at a glance

- **New engine capability.** `GraphStore` gains one **additive** method,
  `bulk_edges_of_type`, that returns all live directed edges of a single type in
  one store-locked scan. It has a provided default of `None`, so every existing
  backend compiles and behaves unchanged until it opts in.
- **Ranked recall builds an adjacency index once.** `recall_facts_ranked` and
  `recall_episodes_ranked` fold the results of two bulk scans
  (`DERIVES_FROM`, `SIMILAR_TO`) into an in-memory adjacency map and run the
  same bounded BFS they always ran — now against RAM, not the database.
- **All-or-nothing capability fold.** If the backend returns `None` for any
  required edge type, the recall falls back to the exact legacy per-node
  neighbour path for that whole recall. Indexed and legacy neighbour lookups are
  never mixed within one recall (that would corrupt scores).
- **Simard consumes it by pin bump.** No Simard call sites change. Simard bumps
  the `amplihack-memory` git rev in `Cargo.toml`, rebuilds `Cargo.lock`, and adds
  `tracing` spans so per-sub-step timing is visible. The
  `"prepared context (…)"` log line is preserved verbatim.

---

## Engine API (`amplihack-memory-lib`)

### `GraphStore::bulk_edges_of_type`

A single additive method on the `GraphStore` trait
(`rust/amplihack-memory/src/graph/protocol.rs`):

```rust
/// Return every *live* directed edge of `edge_type` as `(source, target)`
/// node pairs, in one store-locked scan.
///
/// Returns `None` if the backend does not implement the fast bulk path; the
/// caller then falls back to the legacy per-node neighbour queries with
/// identical results (performance-only fallback, never a correctness or
/// isolation downgrade).
///
/// Contract for overrides:
/// - Include only live edges (apply the same tombstone / not-deleted filter
///   used elsewhere). Soft-deleted, superseded, or archived edges MUST NOT
///   appear.
/// - Preserve direction: the pair is `(source, target)`, matching the
///   directed semantics of the per-node neighbour queries it replaces.
/// - Do NOT apply tenant scoping here. Per-agent isolation is re-applied by
///   the recall layer on every endpoint and every BFS hop.
/// - `edge_type` is validated (`validate_identifier`) before use.
fn bulk_edges_of_type(&self, _edge_type: &str) -> Option<Vec<(GraphNode, GraphNode)>> {
    None
}
```

**Directionality matters.** The two edge types the ranker walks have different
directions, and the index preserves both so BFS semantics match the legacy
`query_neighbors` path exactly:

| Edge type | Direction walked | Meaning |
|---|---|---|
| `DERIVES_FROM` | Outgoing | Provenance / derivation proximity. |
| `SIMILAR_TO` | Both | Semantic-similarity proximity. |

### Backend coverage

| `GraphStore` impl | `bulk_edges_of_type` | Notes |
|---|---|---|
| `LbugGraphStore` (persistent) | **override** | One typed `MATCH ()-[r:REL]->()` scan per matching rel-table (rel tables are already partitioned per type), reusing the existing `tombstone_filter`, acquiring the store lock **once**. Uses the safe typed rel-table iteration (the "dump" path), not the per-node query that triggers lbug issue #100. |
| `InMemoryGraphStore` (test) | **override** | Iterates the in-memory edge map filtered by `edge_type`. Enables deterministic parity and round-trip-counting tests without a persistent DB. |
| `KuzuGraphStore` | default `None` | Inherits legacy per-node path — exact prior behaviour. |
| `HiveGraphStore` | default `None` | Inherits legacy per-node path — exact prior behaviour. |
| `FederatedGraphStore` | default `None` | Inherits legacy per-node path — exact prior behaviour. |

Backends on the default keep working with **no change** and **no regression** —
they simply do not get the speed-up until they add their own override.

### How ranked recall uses the index

`recall_facts_ranked` and `recall_episodes_ranked`
(`rust/amplihack-memory/src/cognitive_memory/ranked.rs`) do this per recall:

1. Load candidate semantic nodes once (as before).
2. Call `bulk_edges_of_type("DERIVES_FROM")` and
   `bulk_edges_of_type("SIMILAR_TO")`.
   - If **either** returns `None`, discard the partial index and take the legacy
     per-node neighbour path for the whole recall (all-or-nothing fold).
3. Build an in-memory adjacency map from the returned pairs, preserving
   direction (`DERIVES_FROM` outgoing, `SIMILAR_TO` both).
4. Run the **same** bounded BFS (`best_edge_score`) the ranker always ran — now
   against the in-memory index instead of the database.
5. Re-apply the **per-agent tenant prune** (`agent_id == agent_name`) on **both
   endpoints and every hop**, and the tombstone exclusion, exactly as the legacy
   path did — the bulk scan is store-wide, so this re-application is what keeps
   tenants isolated.

The weighted score, the descending-score ordering, the phase weighting, the
superseded/archived exclusion, and the human-readable "why this ranked here"
reasons are all unchanged. Episodic recall computes the **same** graph-proximity
term from the **same** two edge scans (`DERIVES_FROM`, `SIMILAR_TO`) and so drops
its own per-node neighbour fan-out. Episode *node* retrieval (`get_episodes`) is
unchanged — the shared adjacency index accelerates only the graph term, not the
node backfill.

---

## Parity guarantees

The change is a pure performance refactor. The following are held byte-identical
between the indexed path and the legacy per-node path, and are pinned by tests:

1. **Identical ordering.** For any fact/episode set and any `RecallWeightSet`,
   the indexed path returns the same items in the same order as the legacy path.
2. **Identical graph term.** Multi-hop BFS proximity scores match, including the
   `DERIVES_FROM` (outgoing) vs. `SIMILAR_TO` (both) direction semantics.
3. **Tenant isolation preserved (critical).** The store-wide bulk scan never
   leaks cross-agent edges into ranking: the `agent_id == agent_name` prune is
   re-applied on both endpoints and every hop.
4. **Tombstones excluded.** Soft-deleted / superseded / archived nodes and edges
   never surface via the index (same `tombstone_filter` / not-deleted rule).
5. **Capability fold is lossless.** A `None` from any backend yields the legacy
   result — a performance-only fallback, never a correctness or isolation change.
6. **No mixed lookups.** Within a single recall, neighbour data comes entirely
   from the index or entirely from legacy queries — never a mix.

Because the six ranked-recall scoring signals (`text_relevance`, `confidence`,
`importance`, `recency`, `usage`, `graph`) and the per-OODA-phase weight table
are unchanged, every guarantee documented in
[Phase-weighted ranked fact recall](./cognitive-memory-ranked-recall.md) and
[Ranked episodic recall](./cognitive-memory-ranked-episodic-recall.md) continues
to hold. This page changes only *how fast* the **graph** signal is computed.

---

## Configuration

There is **no operator configuration and no feature flag.** The fast path is
always on when the active backend implements `bulk_edges_of_type`; otherwise the
legacy path runs transparently. Simard's persistent backend
(`LbugGraphStore`, selected by the `persistent` feature) implements it, so the
daemon gets the speed-up by default.

The set of edge types folded into the adjacency index is fixed to the types the
ranker actually walks:

| Edge type | Purpose in ranking |
|---|---|
| `DERIVES_FROM` | Provenance proximity (also protects facts from pruning — see [provenance](./cognitive-memory-provenance.md)). |
| `SIMILAR_TO` | Semantic-similarity proximity. |

Adding a new proximity edge type to ranking means adding one more
`bulk_edges_of_type` call to the index build; no configuration surface is
exposed for it.

### Dependency pin (Simard)

Simard consumes the engine mechanism through its pinned `amplihack-memory` git
dependency. After the engine PR merges green, Simard advances the rev and
rebuilds the lockfile:

```toml
# Cargo.toml — bumped in lockstep so the final binary links exactly one
# amplihack-memory containing the bulk-adjacency index.
amplihack-memory = { git = "https://github.com/rysweet/amplihack-memory-lib.git", rev = "<engine-commit>", features = ["persistent"] }
```

```bash
# Rebuild the lockfile against the new engine rev (no other manifest edits).
cargo update -p amplihack-memory --precise <engine-commit>
cargo build --release
```

The Simard pin bump must land **after** the engine PR merges, so `Cargo.lock`
never points at an unmerged commit.

---

## Observability

The existing prepare-context summary is preserved verbatim
(`src/ooda_loop/cycle.rs`):

```text
[simard] OODA cycle: Observe complete
[simard] OODA cycle: prepared context (10 facts, 19 triggers, 5 procedures, 5 episodes)
```

Simard adds `tracing` spans around prepare-context and each of its
sub-operations (plus the board reconcile that runs immediately before it in
`ooda_loop::cycle`) so per-sub-step wall-clock is attributable without a
profiler. The spans emit **counts and timings only** — never `node_text`,
`agent_id`, or any memory content (log-hygiene rule SR-9). The `graph_path`
field is **emitted by the engine** (target `amplihack_memory::cognitive_memory`),
not by Simard: the bridge call returns only facts/episodes, so the only way to
surface which path ran is a one-field `tracing` line inside the engine's
`ranked.rs`. That line is **in scope for the engine deliverable** alongside the
trait + BFS refactor — a deliberate, one-line addition, kept because a
performance fix must be verifiable at runtime (see
[Examples](#examples)). If it is dropped from the engine PR, the fast path is
still confirmable by the round-trip-count regression test, only not from a live
log line.

| Span | Wraps | Fields (counts/timings only) |
|---|---|---|
| `prepare_context` | the whole call | `facts`, `triggers`, `procedures`, `episodes`, `elapsed_ms` |
| `recall_facts_ranked` | per-fragment declarative recall | `fragment_count`, `candidate_count`, `elapsed_ms`, `graph_path` (`indexed`\|`legacy`, engine-emitted) |
| `check_triggers` | prospective trigger scan | `triggered`, `elapsed_ms` |
| `recall_procedures` | tokenized procedure recall | `token_count`, `recalled`, `elapsed_ms` |
| `recall_episodes_ranked` | episodic recall | `candidate_count`, `elapsed_ms`, `graph_path` (engine-emitted) |
| `reconcile_board_prospectives` | board→prospective mirror — sibling step in `ooda_loop::cycle`, runs just before prepare-context (not a prepare-context sub-op) | `elapsed_ms` |

`graph_path = indexed` confirms the fast path is active; `graph_path = legacy`
means the backend returned `None` (expected for `Kuzu` / `Hive` / `Federated`,
unexpected for the persistent daemon and worth investigating).

Read the timings live:

```bash
# Follow the per-sub-step spans for one OODA cycle.
RUST_LOG=simard::memory_consolidation=debug,amplihack_memory::cognitive_memory=debug \
  simard daemon | grep -E 'prepare_context|recall_facts_ranked|recall_episodes_ranked|graph_path'
```

---

## Performance characteristics

- **DB round-trips are now constant** in the number of facts: two typed edge
  scans per recall (`DERIVES_FROM`, `SIMILAR_TO`) instead of ~3 per node. At
  ~7,590 facts that is a drop from ~23,000 queries to 2. `prepare-context` runs
  one declarative recall per objective *fragment*, so a cycle issues
  `2 × fragments + 2` scans (the episodic pair) — bounded by objective structure,
  still `O(1)` in fact count. Caching a single adjacency index across a cycle's
  fragment recalls (2 scans/cycle total) is a deliberate future optimization, not
  part of this change.
- **BFS moves to memory.** The graph walk itself is unchanged in shape; it just
  reads the in-memory adjacency map. Per-recall transient memory is `O(E)` for
  the edges of the folded types — acceptable at ~7,590-node scale.
- **Prepare-context wall-clock** targets a fall from ~11 min to **single-digit
  seconds**, so an OODA cycle returns to its intended cadence instead of ~15–18
  min. The change is pinned by a deterministic round-trip-**count** regression,
  not a wall-clock assertion (see [Tests](#tests)), so the exact second-count is
  hardware-dependent and intentionally not asserted.
- **No wall-clock timeouts** were added to any agentic step (policy). The fix is
  algorithmic, not a cap: it removes the work rather than abandoning it.

---

## Tests

### Engine (`amplihack-memory-lib`)

In `rust/amplihack-memory/src/cognitive_memory/tests/ranked_tests.rs`:

- **Bulk == legacy parity.** For the same fixture and weights, the indexed path
  and the legacy per-node path return identical ordering, scores, and reasons —
  including multi-hop BFS and both edge directions.
- **Multi-tenant isolation.** With edges spanning two agents, the indexed path
  surfaces only the querying agent's neighbours; no cross-tenant edge influences
  ranking.
- **Round-trip counting (regression).** A counting `GraphStore` asserts the
  number of store scans per recall is **constant** as the fact count grows into
  the thousands (`O(T)` typed scans, not `O(N)`). This is a deterministic
  count-based assertion — **no wall-clock timeout** — so it cannot flake on CI
  hardware.
- **Persistent smoke test.** `LbugGraphStore::bulk_edges_of_type` returns the
  correct live directed edges and excludes tombstoned ones, exercising the safe
  typed rel-table iteration (not the lbug-#100 per-node path).

### Simard

In `src/cognitive_memory/library_adapter.rs` (or `tests/orchestration_perf.rs`):

- **Orchestration recall-count perf test.** Using `CognitiveMemoryOps` mocks,
  assert prepare-context issues a **constant** number of store scans per cycle
  as fact count grows — proving the orchestration layer does not reintroduce a
  full-store scan on top of the engine fix.
- **`"prepared context (…)"` preserved.** The exact summary log line still fires
  with the same shape.

---

## Examples

### Confirm the fast path is active

```bash
# One cycle, filtered to the graph-path marker. Expect `indexed`.
RUST_LOG=amplihack_memory::cognitive_memory=debug simard daemon 2>&1 \
  | grep -m1 graph_path
# -> recall_facts_ranked{... graph_path=indexed elapsed_ms=42 ...}
```

If you see `graph_path=legacy` on the persistent daemon, the backend returned
`None` from `bulk_edges_of_type` — check that the binary was built with the
`persistent` feature and against the bumped `amplihack-memory` rev.

### Attribute prepare-context time to a sub-step

```bash
RUST_LOG=simard::memory_consolidation=debug simard daemon 2>&1 \
  | grep -E 'prepare_context|recall_facts_ranked|check_triggers|recall_procedures|recall_episodes_ranked|reconcile_board_prospectives'
```

Reading the `elapsed_ms` on each span shows where the (now small) budget goes;
before the fix, `recall_facts_ranked` and `recall_episodes_ranked` dominated the
whole ~11 min.

### Add the fast path to a new backend

Implement one method; the recall layer picks it up automatically:

```rust
impl GraphStore for MyBackend {
    fn bulk_edges_of_type(&self, edge_type: &str) -> Option<Vec<(GraphNode, GraphNode)>> {
        // Validate, scan once under the store lock, apply the live/tombstone
        // filter, preserve (source, target) direction. Return None to keep the
        // legacy per-node path (identical results, just slower).
        Some(self.scan_live_edges(edge_type))
    }
}
```

No recall code changes; the next `recall_facts_ranked` / `recall_episodes_ranked`
builds its adjacency index from your override and logs `graph_path=indexed`.

---

## Invariants

1. **Additive, non-breaking trait change.** `bulk_edges_of_type` has a provided
   `None` default; all `GraphStore` impls compile and behave unchanged until
   they override it.
2. **Byte-identical ranking.** Indexed and legacy paths produce the same order,
   scores, and reasons for any input and any weights.
3. **Constant DB round-trips.** Graph scoring costs a fixed small number of
   typed scans per recall, independent of fact count.
4. **Tenant isolation is never relaxed.** The `agent_id == agent_name` prune is
   re-applied on every endpoint and every hop over the store-wide index.
5. **Tombstones never surface.** The same live/tombstone filter applies to the
   bulk scan as to the per-node path.
6. **All-or-nothing fold.** A recall uses the index for all neighbours or the
   legacy path for all neighbours — never a mix.
7. **No timeouts, no silent degradation.** The fix removes work; it does not cap
   or abandon it, and a `None` fold is a performance-only fallback.
8. **Observability preserved.** The `"prepared context (…)"` summary is
   unchanged; added spans emit only counts and timings.

---

## Related

- [Memory architecture](../memory.md) — operator-level overview
- [Cognitive Memory Architecture](../architecture/cognitive-memory.md) — canonical spec
- [Library-backed Cognitive Memory](../architecture/cognitive-memory-library-adapter.md) — the `amplihack-memory-lib` backend that provides `GraphStore` and ranked recall
- [Phase-weighted ranked fact recall](./cognitive-memory-ranked-recall.md) — the six-signal ranker whose **graph** signal this index will make fast
- [Ranked episodic recall & memory reinforcement](./cognitive-memory-ranked-episodic-recall.md) — episodic recall that reuses the same adjacency index
- [Cognitive-memory provenance](./cognitive-memory-provenance.md) — the `DERIVES_FROM` edges the index walks
