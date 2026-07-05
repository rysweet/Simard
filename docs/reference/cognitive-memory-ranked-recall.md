---
title: Phase-weighted ranked fact recall & snapshot retention
description: How OODA preparation recalls semantic facts with the library's multi-signal ranked recall (relevance + confidence + importance + recency + usage + graph), how Simard tunes the ranking per OODA phase, and how snapshot/goal-record writes deduplicate via CallerKey so repeated records SUPERSEDE instead of accumulating.
last_updated: 2026-07-04
owner: simard
doc_type: reference
related:
  - ../memory.md
  - ../architecture/cognitive-memory.md
  - ../architecture/cognitive-memory-library-adapter.md
  - ./cognitive-memory-ranked-episodic-recall.md
  - ./cognitive-memory-fact-recall.md
  - ./cognitive-memory-preparation-filters.md
  - ./cognitive-memory-goal-store.md
  - ./cognitive-memory-provenance.md
---

# Phase-weighted ranked fact recall & snapshot retention

> Shipped in issue [#2329](https://github.com/rysweet/Simard/issues/2329)
> (`feat(cog-mem): ranked fact recall (phase-weighted) + snapshot
> retention/dedup`). Builds on the PR-A preparation filters
> ([Preparation-phase memory filters](./cognitive-memory-preparation-filters.md))
> and tokenized recall ([Tokenized fact recall in preparation](./cognitive-memory-fact-recall.md)).

Simard's OODA preparation phase no longer gathers `relevant_facts` with a
plain keyword `search_facts`. It now uses the `amplihack-memory-lib`
**ranked recall** (`recall_facts_ranked`), which scores every candidate
fact across six signals and returns them in descending relevance order.
Simard owns the *policy* — a small per-OODA-phase weight table that biases
the ranking toward what each phase cares about — while the library owns the
*mechanism* (the scoring math).

In parallel, periodic **snapshot and goal-record writes** (goal-board
images, per-goal records) now route through the library's **CallerKey
deduplication**. A repeated logical record SUPERSEDES its predecessor (or is
reused unchanged) instead of piling up a new revision every cycle, and a
retention pass reclaims the superseded tail.

The library dependency rev is unchanged (`e3ea136`, `features =
["persistent"]`); both capabilities were already present in the library and
are simply wired through the `CognitiveMemoryOps` trait.

> **Extended by #2395.** The ranked-recall pattern documented here for facts is
> extended to **episodes** in
> [Ranked episodic recall & memory reinforcement](./cognitive-memory-ranked-episodic-recall.md),
> which reuses this page's `RecallWeightSet` and per-OODA-phase weight mapping
> unchanged. #2395 also adds the **usage/recency reinforcement** plumbing the
> ranker scores: a `reinforce_access` seam for recording accesses at the point a
> memory is used (not during preparation, preserving the `record_access = false`
> rule below) and `usage_count` / `last_accessed_at` surfaced on `CognitiveFact`.
> The seam is driven from the point recalled context is surfaced into a cycle's
> prompt (`advance.rs`); per-action attribution is a future refinement.

---

## Ranked recall

### Scoring signals

`recall_facts_ranked` ranks each candidate fact by a weighted sum of six
normalized signals:

| Signal | Meaning |
|---|---|
| **text_relevance** | How well the fact's `concept`/`content` matches the query keywords. |
| **confidence** | The fact's stored confidence (how sure Simard is it is true). |
| **importance** | The fact's stored importance/salience. |
| **recency** | How recently the fact was created or last accessed (exponential decay, 7-day half-life by default). |
| **usage** | How often the fact has been recalled. |
| **graph** | Graph-proximity boost from connected facts (e.g. `DERIVES_FROM` neighbours), up to 1 hop by default. |

Results are returned **already sorted in descending score order** — the
first element is the best match. Simard does not expose the raw numeric
score on `CognitiveFact`; ordering *is* the ranking. Callers that need
"the most relevant N facts" simply take the first N.

### Per-phase weights

Each OODA phase weights those six signals differently. The defaults live in
the Simard-owned `phase_weights::weights_for_phase` mapping (in `ooda_loop`)
and are applied automatically — no operator action is required.

| Phase | text_relevance | confidence | importance | recency | usage | graph | Bias |
|---|---|---|---|---|---|---|---|
| **Observe** | 0.8 | 0.5 | 0.5 | **1.0** | 0.4 | 0.5 | Favor recency — surface what changed lately. |
| **Orient** | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced (library default). |
| **Decide** | 1.0 | **1.0** | 0.6 | 0.3 | 0.3 | 0.5 | Favor confidence/relevance — commit on trusted facts. |
| **Act** | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced. |
| **Sleep** | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced (no prep recall runs in Sleep; included for completeness). |

The **Observe** preset is recency-heavy so that the first thing the brain
sees each cycle is the freshest declarative state. The **Decide** preset is
confidence-heavy so that commitments lean on facts Simard is sure about. The
divergence between these two presets means the *same* fact set can be
ordered differently depending on the phase — that is intentional and is
exercised by the `phase_weights_change_ordering` test.

`Orient`, `Act`, and `Sleep` use the library's balanced default so that the
only deliberate divergences are the two that matter (Observe vs Decide).

### Read-only during preparation

Preparation recall is performed with `record_access = false`. Gathering
`relevant_facts` to build the prepared context does **not** bump a fact's
`usage_count` or `last_accessed_at`. This keeps the recency/usage signals
honest: simply *preparing* a cycle must not make a fact look "recently used"
and skew the next recall in the same cycle. Access is only recorded when a
fact is genuinely consumed elsewhere.

### Superseded facts are never recalled

Preparation recall sets `include_superseded = false` (and
`include_archived = false`), so superseded snapshot revisions never re-enter
the prepared context. This is what lets the CallerKey dedup (below) collapse
the historical snapshot pile-up that the PR-A `goal-board:snapshot` filter
was originally working around.

### What ranked recall replaces (and what it does not)

Ranked recall replaces **only** the per-fragment `relevant_facts` gather in
preparation. The following are deliberately left on the plain `search_facts`
path:

- The exhaustive **goal-fact load** (`GOAL_STORE_FACT_CONCEPT`) — it is a
  full concept load with slug-dedup, not a relevance ranking; ranking it
  would distort which active goals are loaded.
- All existing PR-A filters: the `goal-board:snapshot` drop, the
  `seen_ids` dedup, the stale-slug filter, and the 10-fact truncation cap
  all still apply *after* ranking.

`search_facts` itself is unchanged and remains available for backward
compatibility and for callers that want keyword recall without ranking.

### Recall-precision self-metric (`recall_precision_at_k`)

Ranked recall emits a **recall-quality** signal so the ranking cannot silently
regress. On every `recall_facts_ranked` call the adapter computes
**precision@k** over the returned order and folds it into an in-process running
mean; the OODA daemon drains that mean once per cycle into the durable
`recall_precision_at_k` series in `~/.simard/metrics/metrics.jsonl` (queryable
via `self_metrics::query_metrics` and surfaced on the dashboard `metrics`
endpoint alongside `controlled_forgetting` and the distillation rates).

- **Precision@k** = of the top-`k` returned facts, the fraction that are
  *query-relevant*. Relevance is a coarse keyword proxy: a fact counts when its
  lowercased `concept` or `content` contains at least one query token
  (whitespace-split, punctuation-only tokens like the wildcard `*` dropped).
  This uses the same `.contains` keyword gate as the episodic recall path — it
  is deliberately **broader** than the ranker's exact token/Jaccard
  `text_relevance` score (e.g. `cat` matches `concatenate`), which is acceptable
  for a self-metric baseline: the query itself is the relevance oracle (no
  external labels), and the proxy moves in the same direction as ranking
  quality.
- **Emitted over the returned window.** The durable per-recall value is
  precision@k with `k` = the number of returned facts (precision over the whole
  returned set: "of everything surfaced, how much is on-topic"). The fixed-k
  ordering guarantee (relevant facts occupy the *top* slots) is pinned
  separately by the regression test below at `k = 1, 2`.
- **Undefined, not zero.** A wildcard/empty query (no usable tokens) or an empty
  result set yields `None` and contributes **no** sample, so structural reads do
  not drag the mean toward zero. The emitted per-cycle value is the mean over
  the recalls that *had* a measurable relevance target.
- **Hot-path safe.** The recall path only touches a lock-guarded in-process
  accumulator — no `metrics.jsonl` write per recall, and the whole-memory recall
  lock is released before the metric is folded. The single durable write happens
  once per cycle in the daemon sweep, **unconditionally** (on both successful and
  errored cycles, so a cycle that recalled then errored cannot bleed its samples
  into the next emission). No ranked recall in the window ⇒ no sample emitted.
- **Windowed, cross-source mean.** The emitted value is the mean over every
  ranked fact recall folded since the last drain, across all in-process sources
  sharing the store (OODA preparation, IPC-served recalls, consolidation) — not a
  single-recall or single-source figure. The `samples` count in the metric
  context conveys the volume behind each mean.
- **Pure observation.** Computing/recording precision never changes the returned
  fact set or its order.

The math lives in the pure, deterministic `cognitive_memory::metrics::precision_at_k`
function, and two fixed-corpus regression tests pin the baseline:

- `recall_precision_at_k_baseline` pins the *combined-signal* baseline
  `recall_precision_at_1=1.000 recall_precision_at_2=1.000 recall_precision_at_full=0.500`:
  two topic-relevant facts (also higher-confidence) must occupy the top two slots
  of a four-fact recall.
- `recall_precision_isolates_text_relevance` pins the same top-2 precision on a
  corpus where **every fact has equal confidence**, so `text_relevance` is the
  only signal that can differentiate them. This is the anti-regression teeth: if
  the relevance weight were zeroed or broken, the equal-confidence facts would
  tie on every signal, the relevant pair would no longer be guaranteed on top,
  and precision@2 would fall below `1.0`. (Empirically, the default weights are
  confidence-dominated — a *low*-confidence relevant fact loses to
  high-confidence irrelevant ones — which is exactly why confidence is held
  equal here to isolate the relevance signal.)

---

## Snapshot / goal-record retention (CallerKey dedup)

### The problem

Simard writes the same *kind* of record over and over: a goal-board image
every save, a per-goal record on every `put`. Previously each write created
a brand-new fact node, so the store accumulated dozens of near-identical
revisions that had to be filtered out on every read.

### The mechanism

Snapshot and goal-record writes now carry a stable **caller key**. For a
given key `k`, the library guarantees **at most one live fact**:

- **Identical content** → the existing fact is **reused** (no new node, no
  duplicate).
- **Changed content** → the old fact is **superseded**: it is archived, its
  `superseded_by` is set to the new fact, and a typed `SUPERSEDES` edge is
  created from the new fact to the old one.

Either way the store keeps exactly one live record per logical key, and the
full revision history remains traversable through `SUPERSEDES` edges until
pruned.

### Caller keys

| Logical record | Caller key |
|---|---|
| Goal-board snapshot | `"goal-board:snapshot"` |
| Per-goal record | `format!("goal-store:record:{slug}")` (slug = goal id) |
| Generic fact snapshot | `"<snapshot_kind>"` |

A single stable `goal-board:snapshot` key means every save supersedes the
prior board image. Per-slug goal keys mean each goal's record supersedes its
own previous revision rather than relying solely on the read-side "max
node_id per slug" dedup loop (which remains as a defensive guard). Goal
*carryover* records are out of scope and keep their existing write path.

### Pruning superseded facts

A retention pass reclaims the superseded tail. It runs **non-fatally** in the
consolidation persistence path after the snapshot save — pruning is
housekeeping, so a failure is logged and never aborts teardown.

The retention policy:

| Field | Value | Why |
|---|---|---|
| `include_superseded` | `true` | Reclaim archived/superseded snapshot revisions. |
| `max_facts_per_concept` | `None` | Goal records all share one concept (`GOAL_STORE_FACT_CONCEPT`), so a per-concept cap would evict **live** goal records — not just the superseded tail — once active goals exceed the cap. `include_superseded` already makes the archived tail prunable on its own, so no cap is needed. |
| `ttl_seconds_by_concept` | empty | No time-based eviction; retention here is purely about reclaiming superseded revisions. |
| `min_importance_to_keep` | `0.0` | Never archive live facts on importance grounds (the library's prune test is `importance < min_importance_to_keep`, which can never fire at `0.0`). |
| `dry_run` | `false` | Actually reclaim. |

Provenance-bearing facts (those with `DERIVES_FROM` edges) are protected
from deletion by the library, so pruning superseded snapshots never breaks a
provenance chain. See
[Cognitive-memory provenance](./cognitive-memory-provenance.md).

---

## API (`CognitiveMemoryOps` trait)

Three methods were added to the backend-agnostic `CognitiveMemoryOps` trait.
All three have **default implementations**, so existing implementors and test
mocks compile unchanged; only `LibraryCognitiveMemory` overrides them.

```rust
/// Ranked recall. Returns facts in descending score order.
/// Default impl delegates to `search_facts` (ignores weights), so any
/// non-library backend keeps working with confidence-ranked keyword recall.
fn recall_facts_ranked(
    &self,
    query: &str,
    limit: u32,
    min_confidence: f64,
    weights: RecallWeightSet,
) -> SimardResult<Vec<CognitiveFact>>;

/// Store a fact under a stable caller key. Identical content is reused;
/// changed content supersedes the prior live fact for this key.
/// `caller_key` leads so call sites read `store_fact_with_caller_key(key, …)`,
/// mirroring `store_fact`'s remaining argument order.
/// Default impl delegates to `store_fact` (ignores the key).
fn store_fact_with_caller_key(
    &self,
    caller_key: &str,
    concept: &str,
    content: &str,
    confidence: f64,
    tags: &[String],
    source_id: &str,
) -> SimardResult<String>;

/// Prune superseded/archived facts. Returns the number reclaimed.
/// Default impl is a no-op (`Ok(0)`).
fn prune_superseded(&self) -> SimardResult<usize>;
```

Notes:

- The trait stays `&self`. The library's `recall_facts_ranked` is
  `&mut self` (it *can* record access); the adapter bridges that through its
  existing `Mutex` write-lock pattern.
- The trait takes the Simard-owned `RecallWeightSet`, **not** the library's
  `RecallWeights`, so the trait — and every mock/implementor — stays
  backend-agnostic. The `RecallWeightSet → RecallWeights` conversion is
  adapter-local. `RecallWeightSet` is re-exported from the `cognitive_memory`
  module; the `OodaPhase → RecallWeightSet` mapping lives in `ooda_loop`
  (see below), because only that layer knows about `OodaPhase` and
  `cognitive_memory` must stay a leaf module.
- Ordering is conveyed by result order only — `CognitiveFact` gained no
  score field.

### `RecallWeightSet` and the phase mapping

`RecallWeightSet` is Simard's per-signal weight type — a backend-agnostic
mirror of the library's `RecallWeights` (same six fields, same order). It
lives in `cognitive_memory` so the trait never names a library type:

```rust
pub struct RecallWeightSet {
    pub text_relevance: f64,
    pub confidence: f64,
    pub importance: f64,
    pub recency: f64,
    pub usage: f64,
    pub graph: f64,
}

impl Default for RecallWeightSet {
    // library-balanced default: 1.0, 0.7, 0.5, 0.4, 0.3, 0.6
}
```

The `OodaPhase → RecallWeightSet` mapping lives in `ooda_loop`
(`src/ooda_loop/phase_weights.rs`) — the only layer that knows `OodaPhase`;
`cognitive_memory` must stay a leaf and must not import `ooda_loop`. It is a
free function returning the Simard-owned `RecallWeightSet`:

```rust
// src/ooda_loop/phase_weights.rs
/// Weights for a given OODA phase. Observe favors recency, Decide favors
/// confidence/relevance, and every other phase uses the balanced default.
pub fn weights_for_phase(phase: OodaPhase) -> RecallWeightSet {
    match phase {
        OodaPhase::Observe => /* recency-heavy preset (recency 1.0) */,
        OodaPhase::Decide  => /* confidence-heavy preset (confidence 1.0) */,
        _                  => RecallWeightSet::default(), // balanced
    }
}
```

`cycle.rs` computes the `RecallWeightSet` for the live phase and threads it
*down* into preparation, so the adapter — not the trait — performs the
`RecallWeightSet → RecallWeights` conversion.

The numeric defaults are pinned by a unit test so the documented table
cannot silently drift from the code.

### Phased preparation entry point

A phase-aware variant threads the per-phase weights into preparation. The
existing 3-arg and 4-arg `preparation_memory_operations*` signatures are
kept for backward compatibility and default to the balanced `Orient`
weights.

```rust
pub fn preparation_memory_operations_with_active_slugs_phased(
    objective: &str,
    session_id: &SessionId,
    bridge: &dyn CognitiveMemoryOps,
    active_slugs: Option<&HashSet<&str>>,
    weights: RecallWeightSet,
) -> SimardResult<PreparedContext>;
```

The OODA observe path calls this variant with
`phase_weights::weights_for_phase(OodaPhase::Observe)`.

---

## Examples

### Recall with phase weights

```rust
use crate::ooda_loop::phase_weights::weights_for_phase;

// During OODA Observe — recency-biased: freshest facts first.
let observed = bridge.recall_facts_ranked(
    objective,
    10,                                  // limit
    0.0,                                 // min_confidence
    weights_for_phase(OodaPhase::Observe),
)?;

// During OODA Decide — confidence-biased: trusted facts first.
let decided = bridge.recall_facts_ranked(
    objective,
    10,
    0.0,
    weights_for_phase(OodaPhase::Decide),
)?;
```

Given a recent low-confidence fact `F_new` and an old high-confidence fact
`F_old`, `observed` orders `F_new` before `F_old` (recency wins) while
`decided` orders `F_old` before `F_new` (confidence wins). The two orderings
differ — that is the phase-weighting effect.

### Store a snapshot under a caller key

For the goal-board snapshot the caller key and the `concept` happen to be the
same string (`"goal-board:snapshot"`); they are distinct arguments all the
same — the key drives dedup, the concept is the stored fact's concept.

```rust
// First save: inserts one live snapshot fact.
bridge.store_fact_with_caller_key(
    "goal-board:snapshot",   // caller_key (stable across saves)
    "goal-board:snapshot",   // concept
    &board_json_v1,          // content
    1.0,                     // confidence
    &tags,                   // tags
    session_id,              // source_id
)?;

// Identical save: reused — still exactly one live fact, no duplicate.
bridge.store_fact_with_caller_key(
    "goal-board:snapshot",
    "goal-board:snapshot",
    &board_json_v1,
    1.0,
    &tags,
    session_id,
)?;

// Changed save: supersedes — v1 archived + `superseded_by` set + a
// `SUPERSEDES` edge v2 -> v1; still exactly one live fact.
bridge.store_fact_with_caller_key(
    "goal-board:snapshot",
    "goal-board:snapshot",
    &board_json_v2,
    1.0,
    &tags,
    session_id,
)?;
```

### Prune the superseded tail

```rust
// Runs in the consolidation persistence path, non-fatally.
let reclaimed = bridge.prune_superseded()?;
tracing::debug!("pruned {reclaimed} superseded facts");
```

---

## Invariants

These are the guarantees the implementation upholds (and that the tests
assert):

1. **Single live record per caller key** — after any
   `store_fact_with_caller_key(·, k)`, at most one non-archived fact has
   `dedup_key == k`.
2. **Supersede integrity** — a changed write archives the old fact, sets its
   `superseded_by`, and adds a `SUPERSEDES` edge new → old.
3. **Descending recall order** — `recall_facts_ranked` results are sorted by
   score, highest first.
4. **Phase divergence** — Observe and Decide weights can produce different
   orderings of the same fact set.
5. **Default back-compat** — a backend that does not override
   `recall_facts_ranked` returns the same result as `search_facts` for any
   weights.
6. **Read-only preparation** — preparation recall (`record_access = false`)
   leaves `usage_count` and `last_accessed_at` unchanged.
7. **Superseded never recalled** — preparation recall
   (`include_superseded = false`) never surfaces superseded snapshots.
8. **Recall-precision is monitored** — every `recall_facts_ranked` with a
   measurable relevance target folds `precision@k` into the per-cycle
   `recall_precision_at_k` self-metric; the fixed-corpus baseline
   (`precision@2 == 1.0`, `precision@full == 0.5`) is pinned so a ranking
   regression that demotes relevant facts fails CI.

---

## Related

- [Memory architecture](../memory.md) — operator-level overview
- [Cognitive Memory Architecture](../architecture/cognitive-memory.md) — canonical spec
- [Library-backed Cognitive Memory](../architecture/cognitive-memory-library-adapter.md) — the `amplihack-memory-lib` backend that provides ranked recall and CallerKey dedup
- [Tokenized fact recall in preparation](./cognitive-memory-fact-recall.md) — the keyword `search_facts` path ranked recall builds on
- [Preparation-phase memory filters](./cognitive-memory-preparation-filters.md) — the PR-A snapshot/stale-slug filters that still apply after ranking
- [File-backed goal store](./cognitive-memory-goal-store.md) — the goal records that now dedup via `goal-store:record:{slug}` caller keys
- [Cognitive-memory provenance](./cognitive-memory-provenance.md) — `DERIVES_FROM` edges that protect facts from pruning
