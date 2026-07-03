---
title: Ranked episodic recall & memory reinforcement
description: How OODA preparation recalls past episodes with the library's multi-signal ranked recall (relevance + confidence + importance + recency + usage + graph) instead of a flat keyword scan, how compressed consolidation sources stay recallable through a UNION backfill, and how a usage/recency reinforcement seam (`reinforce_access`) plus `CognitiveFact` usage counters record accesses at the point recalled memories are surfaced into a cycle — not while candidates are merely gathered (per-action attribution remains a future refinement).
last_updated: 2026-07-03
owner: simard
doc_type: reference
related:
  - ../memory.md
  - ../architecture/cognitive-memory.md
  - ../architecture/cognitive-memory-library-adapter.md
  - ./cognitive-memory-ranked-recall.md
  - ./cognitive-memory-episodic-recall.md
  - ./ooda-procedural-memory.md
  - ./cognitive-memory-procedural-idempotency.md
  - ./cognitive-memory-provenance.md
  - ../architecture/episode-distillation.md
---

# Ranked episodic recall & memory reinforcement

> Shipped in issue [#2395](https://github.com/rysweet/Simard/issues/2395)
> (`feat(cog-mem): ranked episodic recall + usage/recency reinforcement`).
> Completes the recall-quality upgrade started for facts in
> [#2329](https://github.com/rysweet/Simard/issues/2329)
> ([Phase-weighted ranked fact recall & snapshot retention](./cognitive-memory-ranked-recall.md))
> by extending the same library-backed ranked-recall pattern to **episodes**,
> and by adding the **usage/recency reinforcement** plumbing — the signal the
> ranker already scores but Simard never recorded — so accesses accrue at the
> point a memory is used. The reinforcement *seam* (`reinforce_access`), its
> fact-level observability, and its wiring at the point recalled memories are
> surfaced into a cycle (the `advance.rs` prompt-injection site) all ship here;
> finer per-action attribution is a future refinement.

`amplihack-memory-lib` exposes a far richer recall surface than Simard was
invoking. #2329 wired the library's multi-signal **ranked recall** into the
OODA preparation phase **for facts only**. Two gaps remained, and this change
closes both:

1. **Episodes** were still recalled with a flat, newest-first keyword scan
   (`search_episodes_by_keywords`) that ignores confidence, importance,
   recency-decay, usage, and the provenance graph. OODA preparation now recalls
   episodes with the library's **`recall_episodes_ranked`**, scored across the
   same six signals and ordered by descending relevance — the *primary*
   under-application this change fixes.
2. **Reinforcement was dead.** The ranker reads a `usage_count` / recency
   signal, but nothing in Simard ever *recorded* an access, so that signal was
   permanently flat for facts and procedures recalled by the OODA loop. A fact's
   reinforcement counters were also invisible at the Simard layer. This change
   adds a single **reinforce-at-use** seam (`reinforce_access`), surfaces the
   counters on `CognitiveFact`, and **drives the seam** from the point the
   recalled context is surfaced into a cycle's prompt (`advance.rs`), so accesses
   accrue automatically. Reinforcing *which* recalled memory specifically drove
   the committed action (rather than all surfaced memories) is a future
   refinement (see
   [Memory reinforcement](#memory-reinforcement-usagerecency-learning)).

As of issue #2395 the library dependency rev was unchanged (`285de92`, `lbug = "=0.15.4"`,
`features = ["persistent"]`); both capabilities were already present in the
library and are simply wired through the `CognitiveMemoryOps` trait. That change made
**no** `Cargo.toml` change and **no** on-disk store-format change. (The #2329
fact-recall doc was written against an earlier pin, `e3ea136`; #2395 reflected
`285de92` — the ranked-recall and `record_access` APIs are present in
both.) The pin has since advanced to `26d49bf8` (`lbug = "=0.17.1"`) in the
de-fork Phase 2b ([#2307](https://github.com/rysweet/Simard/issues/2307)).

---

## Ranked episodic recall

### Scoring signals

`recall_episodes_ranked` ranks each candidate episode by a weighted sum of the
same six normalized signals already used for ranked fact recall:

| Signal | Meaning for an episode |
|---|---|
| **text_relevance** | How well the episode's `content` matches the query keywords. |
| **confidence** | The episode's stored confidence/salience. |
| **importance** | The episode's stored importance. |
| **recency** | How recently the episode occurred or was last accessed (exponential decay, 7-day half-life by default). |
| **usage** | How often the episode has been recalled. |
| **graph** | Graph-proximity boost from connected nodes — `DERIVES_FROM` / `SIMILAR_TO` neighbours that tie an episode to the facts and procedures distilled from it. |

Results are returned **already sorted in descending score order** — the first
element is the best match. As with ranked fact recall, Simard does not surface
the raw numeric score on `CognitiveEpisode`; **ordering *is* the ranking**, so
no score field is added to the DTO. Callers that need "the most relevant N
episodes" simply take the first N.

This replaces the previous behaviour where `search_episodes_by_keywords`
returned episodes strictly **newest-first** (descending `temporal_index`), giving
every keyword hit equal weight and letting a stale-but-recent episode outrank a
highly relevant older one.

### Relevance gate (keyword-scoped ranking)

The library's `recall_episodes_ranked` scores **every** non-compressed episode,
including by recency — so a recent but topically-unrelated episode earns a
non-zero score and would surface. Simard's episodic recall is **relevance-gated**
(an objective recalls only episodes that share a keyword with it; an unrelated
objective recalls *nothing*), and that contract is preserved: the adapter gates
the ranked output to episodes whose `content` contains at least one query keyword
(case-insensitive substring, matching `search_episodes_by_keywords`). The gate is
applied **before** truncation, so a relevant episode ranked behind recent noise
is not dropped before the gate runs. The net effect is "rank the keyword-relevant
episodes" — the multi-signal ranking upgrades the *ordering* among relevant
candidates without widening the *set* beyond what lexical recall would have
returned.

### Per-phase weights (shared with fact recall)

Episodic recall is tuned by the **same** Simard-owned per-OODA-phase weight
table that drives ranked fact recall — there is no second policy to maintain.
The `RecallWeightSet` computed for the live phase (via
`phase_weights::weights_for_phase`, in `ooda_loop`) is threaded into preparation
and applied to **both** the fact and the episode recall in that pass, so the two
recall streams agree on what the current phase cares about.

| Phase | text_relevance | confidence | importance | recency | usage | graph | Bias |
|---|---|---|---|---|---|---|---|
| **Observe** | 0.8 | 0.5 | 0.5 | **1.0** | 0.4 | 0.5 | Favor recency — surface what happened lately. |
| **Orient** | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced (library default). |
| **Decide** | 1.0 | **1.0** | 0.6 | 0.3 | 0.3 | 0.5 | Favor confidence/relevance — commit on trusted recollection. |
| **Act** | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced. |
| **Sleep** | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced (no prep recall runs in Sleep). |

Because Observe is recency-heavy and Decide is confidence-heavy, the *same*
episode set can be ordered differently depending on the phase — that divergence
is intentional and is exercised by a test (see [Invariants](#invariants)). See
[Phase-weighted ranked fact recall](./cognitive-memory-ranked-recall.md#per-phase-weights)
for the canonical definition of `RecallWeightSet` and the `OodaPhase` mapping;
this feature reuses them unchanged.

### Read-only during preparation

Like ranked fact recall, episodic recall in preparation runs with
**`record_access = false`**. Gathering episodes to build the prepared context
does **not** bump an episode's usage/recency counters. This keeps the signals
honest: within a single preparation pass the brain issues several recalls
(per-fragment facts, procedures, episodes), and a recall that mutated state
would skew the ordering a later recall in the *same* cycle sees, and inflate the
usage signal N× per objective. Reinforcement is deferred to the point a recalled
memory is genuinely **used** — see [Memory reinforcement](#memory-reinforcement-usagerecency-learning).

### Compressed consolidation sources stay recallable (UNION backfill)

This is the one place ranked episodic recall is **not** a drop-in swap.

The library's `recall_episodes_ranked` **skips episodes flagged
`compressed`** — episodes that have been folded into a distilled summary. But
Simard has a standing contract (defended by the #2298 idempotency and the
distillation tests) that compressed **source** episodes must remain recallable,
so that a distilled fact or procedure can always be traced back to — and
re-grounded in — the raw episodes it came from. The pinned library rev exposes
**no flag** to include compressed episodes in the ranked path.

Simard therefore composes the two halves with a **UNION backfill**:

```
recall_episodes_ranked(query, limit, weights)             ── ranked LIVE episodes
        then keep only keyword-relevant (relevance gate)
        ∪
get_episodes(.., include_compressed=true)                 ── compressed SOURCES
        then keep only `e.compressed == true` AND keyword-relevant
        │
        ▼
merge by node_id  →  ranked live episodes first, then compressed sources
        │              (de-duplicated; a node_id never appears twice)
        ▼
truncate to limit
```

- **Primary stream** — `recall_episodes_ranked` over the live (non-compressed)
  set, fully ranked and phase-weighted, then narrowed by the keyword
  relevance gate above.
- **Backfill stream** — a scan of `get_episodes(.., true)` (which yields live
  **and** compressed episodes) filtered to `e.compressed == true` and the same
  keyword relevance, so it yields only the compressed consolidation sources the
  ranker dropped. There is no pre-existing "compressed-only" recall mode — the
  `compressed` filter is new to this path.
- **Merge** — concatenate by `node_id`, ranked live episodes first, then
  compressed sources, drop any `node_id` already present, and truncate to
  `limit`.

This preserves the upgraded ranking for everyday (live) episodes **and** the
"sources remain recallable" guarantee at the same time. The alternative of
dropping compressed episodes entirely was rejected because it would regress the
#2298 / distillation contracts; the alternative of a library include-compressed
flag was rejected because it would require bumping the pinned rev (out of scope).

### Self-session noise filter (unchanged)

The existing self-session noise filter is preserved verbatim and applied
**after** ranked recall returns: episodes whose `source_label` begins with
`session-` are dropped inside `preparation_memory_operations`, because they are
the current session loop's own breath echoing back into the prompt. Episodes
from `goal-curator`, `consolidation`, `distill:…`, and meeting probes pass
through. See
[Episodic recall in preparation → Self-session noise filter](./cognitive-memory-episodic-recall.md#self-session-noise-filter).

### What ranked episodic recall replaces (and what it does not)

Ranked recall replaces **only** the `episodic_recall` gather in OODA
preparation (`memory_consolidation/mod.rs`). The following are deliberately
unchanged:

- **`search_episodes_by_keywords`** — the flat keyword scan is **kept**. It is
  still the default trait implementation (so non-library backends keep working),
  and it is reused (with an added `e.compressed` filter) as the compressed-source
  backfill matcher above. Operators and callers that want a plain newest-first
  keyword scan still have it.
- **`check_triggers`** and all prospective-memory ("fires-once") semantics — a
  different concept from episodic recall, left entirely alone.
- The **`## Prior episodes`** prompt-injection block, its truncation, the
  empty-section omission, and the `episodes recalled (N raw, M session-filtered)`
  observability line — all preserved. Only the *ordering* of the episodes inside
  that block changes (ranked, not newest-first).

---

## Memory reinforcement (usage/recency learning)

### Governing principle: preparation reads, use reinforces

The ranker scores a **usage** and a **recency** signal, which only mean
anything if accesses are actually recorded. The question is *where*. The answer
follows directly from the read-only-preparation rule above:

> **Recall during OODA *preparation* is a pure read — it must not reinforce.**
> Reinforcement belongs at the point a memory is actually **used / acted upon**,
> after Decide/Act, not where candidates are gathered.

Before this change, Simard recorded an access **nowhere** — so the usage signal
was permanently flat and procedure reinforcement was dead (a stored procedure's
`usage_count` never moved off its store-time value). This change adds exactly
one reinforce-at-use seam and **drives it** at the point the recalled context is
surfaced into a cycle's prompt, so facts, procedures, and episodes are reinforced
once they are actually put in front of the agent.

### `reinforce_access` — the single reinforcement seam

A new trait method records that a recalled memory was used:

```rust
/// Record that `node_id` (a fact, episode, or procedure) was actually used,
/// bumping its `usage_count` and `last_accessed_at` via the library's
/// `record_access`. Driven from the point of use (`advance.rs`), never from
/// preparation.
///
/// Default impl is a no-op (`Ok(())`) so non-library backends keep compiling.
fn reinforce_access(&self, node_id: &str, kind: MemoryKind) -> SimardResult<()>;
```

`MemoryKind` selects the memory family (`Fact`, `Episode`, `Procedure`) so the
adapter dispatches to the right library store (and, for facts, strips the
adapter's monotonic sequence prefix off the recall-surfaced `node_id` before
recording). Reinforcement is **best-effort**: a failure is logged and never
aborts the OODA cycle, matching every other memory call in `cycle.rs`.

The seam is the **mechanism**; this change also **drives** it. When the OODA
goal-session path (`ooda_actions/goal_session/advance.rs`) flattens the prepared
context into the agent's prompt, it then calls
`memory_consolidation::reinforce_prepared_context`, which records an access for
**every** recalled fact, procedure, and episode it just surfaced — so memories
that repeatedly prove relevant climb the **usage** signal and are surfaced
earlier by later ranked recalls. Reinforcement is deliberately kept out of
*preparation* (a pure read) and placed at this point-of-use instead. A finer
attribution — recording an access only for the specific recalled memory that
drove the committed `ActionKind`, rather than all surfaced memories — is a future
refinement, because the Act path does not yet track which memory grounded the
action.

### Fact reinforcement is now observable

`CognitiveFact` gains two reinforcement counters, mapped from the library's
`SemanticFact`, so reinforcement is **visible** at the Simard layer (to tests,
`simard memory dump`, and the dashboard):

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CognitiveFact {
    pub node_id: String,
    pub concept: String,
    pub content: String,
    pub confidence: f64,
    pub source_id: String,
    pub tags: Vec<String>,
    pub usage_count: i64,                          // NEW (#2395)
    pub last_accessed_at: Option<DateTime<Utc>>,   // NEW (#2395)
}
```

Both fields are additive. `CognitiveFact` keeps its
`Clone, Debug, PartialEq, Serialize, Deserialize` derives, and there are **no
whole-struct equality assertions** in the suite, so the addition is
PartialEq-safe; the ~22 struct-literal construction sites add the two fields
mechanically (`usage_count: 0, last_accessed_at: None` for fixtures). The
companion `CognitiveProcedure` already carries `usage_count`, and
`CognitiveEpisode` is unchanged.

### Procedure reinforcement and usage-ordered recall

Procedure recall during preparation stays a **pure read** (per the governing
principle) and is **already ordered by usage strength** — the library's
procedure search returns rows sorted by `usage_count` descending, so
frequently-useful procedures already lead. The previously-stale claim that
preparation procedure recall was a "single-CONTAINS on name" newest-first scan
is corrected: the library matches the query against the procedure **name OR its
steps** and sorts by usage.

The previously-missing piece was the **increment at use**, and this change adds
it. `reinforce_access(node_id, MemoryKind::Procedure)` is now driven by
`reinforce_prepared_context` (from `advance.rs`) whenever a recalled procedure is
surfaced into a cycle's prompt, bumping that procedure's `usage_count` /
`last_accessed_at`. Over successive cycles the procedures that keep proving
relevant rise to the top of the usage-ordered recall — so the procedural store
*learns* instead of freezing at its store-time counts. This composes with the
#2298 exact-name idempotency
([Procedural-memory store idempotency](./cognitive-memory-procedural-idempotency.md)),
which already bumps usage at **store** time when an identical procedure is
re-derived; reinforce-at-use adds the **use**-time bump so both paths feed the
same signal.

---

## API (`CognitiveMemoryOps` trait)

Two methods are added to the backend-agnostic `CognitiveMemoryOps` trait, and
`CognitiveFact` gains two fields. Both new methods have **default
implementations**, so existing implementors and test mocks (legacy Python
bridge, IPC client, in-memory mocks) compile unchanged; only
`LibraryCognitiveMemory` overrides them.

```rust
/// Ranked episodic recall. Returns episodes in descending score order,
/// scored across text-relevance + confidence + importance + recency + usage
/// + graph proximity, with the per-OODA-phase `weights` applied.
///
/// The default implementation delegates to `search_episodes_by_keywords`
/// (splitting `query` on whitespace, ignoring `weights`) so non-library
/// backends keep working with the flat newest-first keyword scan. Only
/// `LibraryCognitiveMemory` overrides it to call the library's
/// `recall_episodes_ranked` (with `record_access = false`) and apply the
/// compressed-source UNION backfill.
fn recall_episodes_ranked(
    &self,
    query: &str,
    limit: u32,
    weights: RecallWeightSet,
) -> SimardResult<Vec<CognitiveEpisode>> {
    let tokens: Vec<String> =
        query.split_whitespace().map(str::to_string).collect();
    self.search_episodes_by_keywords(&tokens, limit)
}

/// Record that a recalled memory was actually used, reinforcing its usage /
/// recency counters via the library's `record_access`. Driven from the point of
/// use (`advance.rs`), never from preparation.
///
/// Default impl is a no-op (`Ok(())`).
fn reinforce_access(&self, node_id: &str, kind: MemoryKind) -> SimardResult<()> {
    let _ = (node_id, kind);
    Ok(())
}
```

Notes:

- The trait stays `&self`. The library's `recall_episodes_ranked` and
  `record_access` are `&mut self` (they *can* mutate); the adapter bridges that
  through its existing `Mutex` write-lock — durability and recovery are
  unchanged because every touched method already write-locks.
- The trait takes the Simard-owned `RecallWeightSet`, **not** the library's
  `RecallWeights`, so the trait — and every mock — stays backend-agnostic. The
  `RecallWeightSet → RecallWeights` conversion is adapter-local and identical to
  the fact-recall path.
- Ordering is conveyed by result order only — `CognitiveEpisode` gained **no**
  score field.
- `MemoryKind` is a small Simard-owned enum (`Fact | Episode | Procedure`) in
  `cognitive_memory`; it names no library type.
- `reinforce_access` ships as the reinforcement seam — the trait method, the
  adapter dispatch by `MemoryKind`, and the mapping onto the library's
  `record_access` — and is **driven** at the point of use by
  `memory_consolidation::reinforce_prepared_context` (called from `advance.rs`
  once the recalled context is surfaced into the prompt). Per-action attribution
  (only the memory that drove the committed action) is a future refinement.

### Preparation wiring

OODA preparation swaps the episode gather from the flat keyword scan to ranked
recall, threading the same phase weights already flowing to `recall_facts_ranked`:

```rust
// memory_consolidation/mod.rs — episodic recall gather
let (raw_recall_count, session_filtered_count, episodic_recall) =
    if tokens.is_empty() {
        (0, 0, Vec::<CognitiveEpisode>::new())   // unchanged short-circuit
    } else {
        // was: bridge.search_episodes_by_keywords(&tokens, 5)?
        // The query is the space-joined keyword tokens, so the non-library
        // default impl re-splits it into exactly the filtered keyword set.
        let query = tokens.join(" ");
        let raw = bridge.recall_episodes_ranked(&query, 5, weights)?;
        let raw_len = raw.len();
        let kept: Vec<CognitiveEpisode> = raw
            .into_iter()
            .filter(|e| !e.source_label.starts_with("session-")) // preserved
            .collect();
        (raw_len, raw_len - kept.len(), kept)
    };
```

The empty-token short-circuit, the `session-` self-noise filter, the `raw` /
`session-filtered` counts, and the prompt block are all preserved; only the
recall call and its ordering change.

---

## Examples

### Ranked episode recall by phase

```rust
use crate::ooda_loop::phase_weights::weights_for_phase;

// During OODA Observe — recency-biased: freshest relevant episodes first.
let observed = bridge.recall_episodes_ranked(
    objective,
    5,                                       // limit
    weights_for_phase(OodaPhase::Observe),
)?;

// During OODA Decide — confidence-biased: most trusted recollection first.
let decided = bridge.recall_episodes_ranked(
    objective,
    5,
    weights_for_phase(OodaPhase::Decide),
)?;
```

Given a recent low-relevance episode `E_new` and an older highly-relevant
episode `E_old`, `observed` may order `E_new` first (recency wins) while
`decided` orders `E_old` first (relevance/confidence wins). The flat
`search_episodes_by_keywords` would have returned `E_new` first in **both** cases
purely because it is newer.

### Compressed source still recalled after ranking

```rust
// epi_src was distilled into a fact and flagged `compressed = true`.
// The library ranker skips it; the UNION backfill recovers it.
let recalled = bridge.recall_episodes_ranked("auth null check", 5, weights)?;
assert!(
    recalled.iter().any(|e| e.node_id == epi_src.node_id),
    "compressed consolidation source must remain recallable",
);
```

### Reinforce a memory at the use point

```rust
// The reinforce-at-use seam. In the live loop it is driven by
// `reinforce_prepared_context` (from `advance.rs`) for every recalled memory
// surfaced into the cycle's prompt; it can also be called directly, e.g. after a
// recalled fact and procedure grounded a committed action:
bridge.reinforce_access(&used_fact.node_id, MemoryKind::Fact)?;
bridge.reinforce_access(&used_procedure.node_id, MemoryKind::Procedure)?;

// A later recall of the same fact now shows the reinforcement:
let again = bridge.recall_facts_ranked(query, 10, 0.0, weights)?;
let f = again.iter().find(|f| f.node_id == used_fact.node_id).unwrap();
assert!(f.usage_count >= 1);
assert!(f.last_accessed_at.is_some());
```

### Preparation is read-only (no reinforcement skew)

```rust
// Two successive preparation passes over an unchanged store recall episodes
// in the SAME order — preparing a cycle never reinforces.
let first  = preparation_memory_operations(objective, &session, bridge)?;
let second = preparation_memory_operations(objective, &session, bridge)?;
assert_eq!(
    ids(&first.episodic_recall),
    ids(&second.episodic_recall),
    "preparation recall must be a pure read (record_access = false)",
);
```

---

## Invariants

These are the guarantees the implementation upholds (and that the tests
assert):

1. **Descending recall order** — `recall_episodes_ranked` returns episodes
   sorted by score, highest first (not merely newest-first).
2. **Phase divergence** — Observe and Decide weights can produce different
   orderings of the same episode set (parity with the fact-recall
   `phase_weights_change_ordering` test).
3. **Preparation uses ranked episodes** — the OODA preparation step's
   `episodic_recall[0]` is the relevance/phase winner, not merely the newest
   episode.
4. **Read-only preparation** — two successive preparation recalls over an
   unchanged store yield identical episode ordering; preparation never bumps
   `usage_count` / `last_accessed_at` (`record_access = false`).
5. **Compressed sources preserved** — a compressed consolidation-source episode
   whose keywords match the objective still appears in the recall result after
   the ranked switch (regression guard for #2298 / distillation).
6. **Default back-compat** — a backend that does not override
   `recall_episodes_ranked` returns the same result as
   `search_episodes_by_keywords` for the whitespace-split query.
7. **Reinforcement seam, driven at use** — calling `reinforce_access(node_id,
   kind)` increments that memory's `usage_count` and advances `last_accessed_at`;
   a subsequent ranked recall observes the change, and `CognitiveFact` surfaces
   both counters. The seam is driven in the live loop by
   `reinforce_prepared_context` once the recalled context is surfaced into a
   cycle's prompt (preparation itself stays a pure read). Per-action attribution
   is a future refinement.
8. **Self-session filter preserved** — `session-` episodes are still dropped
   after recall; the `raw` / `session-filtered` observability counts are
   unchanged in meaning.
9. **Durability unchanged** — every reinforcing/ranked call routes through the
   adapter's existing write-lock; WAL/checkpoint/recovery semantics are
   untouched.

---

## Related

- [Memory architecture](../memory.md) — operator-level overview of the six
  memory types, consolidation, and recall
- [Phase-weighted ranked fact recall & snapshot retention](./cognitive-memory-ranked-recall.md) —
  the #2329 fact-side feature this extends; canonical `RecallWeightSet` and the
  `OodaPhase` weight mapping reused here
- [Episodic recall in preparation](./cognitive-memory-episodic-recall.md) — the
  flat keyword path that ranked recall replaces in preparation (and that remains
  the default trait impl / compressed-source backfill)
- [OODA procedural memory](./ooda-procedural-memory.md) — procedure storage and
  the usage-ordered recall that reinforce-at-use will feed once the Act-path seam
  is wired
- [Procedural-memory store idempotency](./cognitive-memory-procedural-idempotency.md) —
  exact-name dedup that bumps usage at store time; complements reinforce-at-use
- [Cognitive-memory provenance](./cognitive-memory-provenance.md) —
  `DERIVES_FROM` / `SIMILAR_TO` edges the graph-proximity signal traverses, and
  the consolidation sources the UNION backfill protects
- [Library-backed Cognitive Memory](../architecture/cognitive-memory-library-adapter.md) —
  the `amplihack-memory-lib` adapter that provides `recall_episodes_ranked` and
  `record_access`
- [Cognitive Memory Architecture](../architecture/cognitive-memory.md) —
  canonical spec
