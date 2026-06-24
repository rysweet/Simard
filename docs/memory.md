---
title: Memory architecture
description: Top-level overview of Simard's six-type cognitive memory, consolidation flow, ranked fact and episodic recall, usage/recency reinforcement, snapshot retention, and on-disk layout. Cross-links to the canonical architecture page.
last_updated: 2026-06-23
owner: simard
doc_type: concept
---

# Memory architecture

Simard's memory is not a flat key-value store. She uses **six distinct memory types** modeled after cognitive psychology. They are provided by the upstream [`amplihack-memory-lib`](https://github.com/rysweet/amplihack-memory-lib) crate (persistent, LadybugDB/`lbug`-backed) and reached through the `LibraryCognitiveMemory` adapter, which implements the `CognitiveMemoryOps` trait. This library backend is the sole on-disk cognitive-memory backend — there is no Python bridge and no native fork.

For the full canonical specification (schema, consolidation rules, hive event bus contract) see [Cognitive Memory Architecture](architecture/cognitive-memory.md). This page is the operator-level summary.

## The six memory types

| Type | Lifetime | What it holds |
|------|----------|---------------|
| **Sensory** | TTL ~300 s (configurable) | Raw observations: PTY output, error messages, objective text. Auto-expires unless promoted. |
| **Working** | Task-scoped (cleared at task end) | The 20-slot active task context: goal, constraints, plan steps, current execution state. |
| **Episodic** | Persistent, autobiographical | "What happened this session" — every cycle, every action, every observation. |
| **Semantic** | Persistent, deduplicated | Facts and learned concepts promoted from episodic memory ("the test harness uses CARGO_TARGET_DIR"). |
| **Procedural** | Persistent, indexed by trigger, deduplicated by name | Learned how-to: action sequences that worked for a given situation. Written by the OODA Act phase for successful outcomes. Storing an identically-named procedure is idempotent (#2298). See [OODA procedural memory](reference/ooda-procedural-memory.md) and [Procedural-memory store idempotency](reference/cognitive-memory-procedural-idempotency.md). |
| **Prospective** | Persistent, time/event-indexed | Future intentions: Active goals as trigger-action pairs, meeting action items. See [Goal–prospective memory mirror](reference/goal-prospective-memory-mirror.md). |

## Consolidation flow

```
(intake)  ──(classify)───▶  Episodic    (noise dropped/down-scoped at the door, #2327)
Sensory   ──(attention)──▶  Episodic
Working   ──(task end)───▶  Episodic
Episodic  ──(distill)────▶  Semantic    (DERIVES_FROM edge back to source episode, #2325)
Episodic  ──(distill)────▶  Procedural  (PROCEDURE_DERIVES_FROM edge, #2327)
OODA Act  ──(success)────▶  Procedural    (#2280)
Goal put  ──(Active)─────▶  Prospective   (#2207/#2280)
```

A deterministic **episode ingestion policy** runs before every
`store_episode` write: it drops operational-noise episodes (session
start/complete/persist markers, `flushing working memory`,
`continue_skipping`) and down-scopes the unrecognised, while storing
meaningful events with structured metadata — unless a failure signal
overrides the drop (#2327). Promotion then runs **automatically** at the
end of every OODA cycle (on a backlog threshold or cycle interval, not
only when the brain chooses `ConsolidateMemory`), distilling recurring
episodes into both facts and procedures. See
[Episode ingestion policy & automatic promotion](architecture/episode-ingestion-policy.md).

Facts (and procedures) written *with provenance* keep a typed
`DERIVES_FROM` / `PROCEDURE_DERIVES_FROM` graph edge back to the
episode(s) they were derived from, turning the flat node store into a
connected graph that can be traversed both ways (#2325). See
[Cognitive-memory provenance](reference/cognitive-memory-provenance.md).

The OODA daemon dispatches a `consolidate-memory` action whenever working-memory pressure or recent-episode density crosses a threshold. Consolidation is idempotent and runs without spawning an engineer subprocess. Procedural memories are written inline during the OODA Act phase (not during consolidation) — each successful `ActionOutcome` produces an `ooda:{kind}` procedure. Prospective memories are written each cycle by a **board-sourced reconcile**: before every preparation pass the daemon mirrors each Active goal in the live `GoalBoard` into a prospective trigger via `store_prospective`, so `check_triggers` has something to match. See [Goal–prospective memory mirror](reference/goal-prospective-memory-mirror.md) for the original `CognitiveMemoryGoalStore` mirror and [Goal-board prospective reconcile](reference/goal-board-prospective-reconcile.md) for the per-cycle board-sourced step that the live daemon actually runs.

## Ranked fact recall & retention

Shipped in issue [#2329](https://github.com/rysweet/Simard/issues/2329). Two
coordinated changes wire already-available `amplihack-memory-lib` capabilities
into Simard's cognitive-memory layer. The library dependency rev is unchanged
(`e3ea136`).

### Ranked recall in preparation

The OODA preparation phase gathers `relevant_facts` with the library's
**ranked recall** (`recall_facts_ranked`) instead of a plain keyword
`search_facts`. Every candidate fact is scored across six signals —
**text relevance + confidence + importance + recency + usage + graph
proximity** — and returned in **descending score order** (the first fact is
the best match; ordering *is* the ranking, so no numeric score is added to
`CognitiveFact`).

Simard owns *which* signals matter per OODA phase via the
`phase_weights::weights_for_phase` mapping (in `ooda_loop`). Defaults — fields
are `(text_relevance, confidence, importance, recency, usage, graph)`:

| Phase | text_rel | confidence | importance | recency | usage | graph | Bias |
|---|---|---|---|---|---|---|---|
| **Observe** | 0.8 | 0.5 | 0.5 | **1.0** | 0.4 | 0.5 | Favor recency — what changed lately. |
| **Orient** | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced (library default). |
| **Decide** | 1.0 | **1.0** | 0.6 | 0.3 | 0.3 | 0.5 | Favor confidence/relevance for commitments. |
| **Act** | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced. |
| **Sleep** | 1.0 | 0.7 | 0.5 | 0.4 | 0.3 | 0.6 | Balanced (no prep recall in Sleep). |

Observe is recency-biased so the brain sees the freshest state first; Decide
is confidence-biased so commitments lean on trusted facts. The divergence
means the same fact set can be ordered differently per phase. Preparation
recall runs with `record_access = false`, so merely preparing a cycle does
**not** bump a fact's usage/recency and skew later recalls. Superseded
snapshots are never recalled (`include_superseded = false`). The plain
`search_facts` path is unchanged and still backs the exhaustive goal-fact
load and the PR-A filters, which apply *after* ranking.

### Snapshot / goal-record retention (CallerKey dedup)

Periodic snapshot and goal-record writes (goal-board images, per-goal
records) route through the library's **CallerKey dedup**. For a stable caller
key, the library keeps **at most one live fact**: identical content is
**reused** (no duplicate), changed content **supersedes** the prior fact
(old archived, `superseded_by` set, typed `SUPERSEDES` edge new → old).

| Logical record | Caller key |
|---|---|
| Goal-board snapshot | `"goal-board:snapshot"` |
| Per-goal record | `format!("goal-store:record:{slug}")` (slug = goal id) |

This collapses the historical snapshot pile-up that the PR-A
`goal-board:snapshot` filter was working around. A retention pass
(`prune_superseded`) reclaims the superseded tail; it runs **non-fatally** in
the consolidation persistence path and protects provenance-bearing facts.

See [Phase-weighted ranked fact recall & snapshot retention](reference/cognitive-memory-ranked-recall.md)
for the full API, examples, and invariants.

## Ranked episodic recall & reinforcement

Shipped in issue [#2395](https://github.com/rysweet/Simard/issues/2395). This
extends the #2329 ranked-recall pattern from facts to **episodes**, and turns on
the **usage/recency reinforcement** the ranker had always scored but Simard
never recorded. The library dependency rev is unchanged (`285de92`, `lbug`
pinned at `0.15.4`) — both capabilities were already present in the library and
are simply wired through the `CognitiveMemoryOps` trait.

### Ranked recall for episodes

OODA preparation no longer recalls past episodes with a flat, newest-first
keyword scan (`search_episodes_by_keywords`). It now uses the library's
**`recall_episodes_ranked`**, scoring every candidate episode across the same
six signals as facts — **text relevance + confidence + importance + recency +
usage + graph proximity** — and returning them in **descending score order**.
The per-OODA-phase `RecallWeightSet` already computed for fact recall is threaded
into episode recall too, so a recency-biased Observe and a confidence-biased
Decide can order the same episodes differently. Preparation recall stays a pure
read (`record_access = false`), and the existing `session-` self-noise filter and
`## Prior episodes` prompt block are preserved — only the *ordering* improves.

**Compressed sources stay recallable.** The library ranker skips `compressed`
episodes (those folded into a distilled summary), but Simard must keep
consolidation **sources** recallable (the #2298 / distillation contract). Ranked
episode recall therefore UNIONs the ranked live episodes with a compressed-only
keyword backfill, merged by `node_id`, so live episodes get the upgraded ranking
**and** distilled sources remain traceable.

### Reinforcement at the point of use

Recall during preparation is a pure read, so reinforcement belongs where a memory
is actually **used** — when the recalled context is surfaced into a cycle's
prompt — via a single `reinforce_access(node_id, kind)` seam over the library's
`record_access`. This change ships that seam, makes fact reinforcement
**observable** (`CognitiveFact` now carries `usage_count` and `last_accessed_at`),
and **drives** it: when the goal-session path (`advance.rs`) flattens the prepared
context into the prompt, it calls `reinforce_prepared_context`, which records an
access for every recalled fact, procedure, and episode it surfaced. So repeatedly
useful facts and procedures climb the **usage** signal and surface earlier next
time. Procedure recall is already usage-ordered, so it feeds directly off that
signal. Recording an access only for the *specific* memory that drove a committed
action — rather than all surfaced memories — is a future refinement.

See [Ranked episodic recall & memory reinforcement](reference/cognitive-memory-ranked-episodic-recall.md)
for the full API, the UNION backfill, examples, and invariants.

## Inspecting memory from the CLI

Use `simard memory stats` to see per-type counts for the live store, and
`simard memory dump` for sample rows. Both are read-only and safe to run
while the daemon holds the store — they read through the daemon's memory
socket when it is up and fall back to a direct on-disk open when it is
down.

```text
$ simard memory stats
cognitive memory @ /home/azureuser/.simard/cognitive  (via daemon socket)

  TYPE          COUNT
  sensory           4
  working           7
  episodic         18
  semantic          5     (facts)
  procedural        5     (procedures)
  prospective       5     (triggers)
  ---------------------
  total            44

edges / connections:
  (edges: run with daemon stopped for graph stats)
```

### Edges / connections (graph-edge + dedup introspection)

Per-type counts show how many *nodes* live in each memory; the **edges /
connections** section (issue #2331) shows how those nodes are *wired together*,
so an operator can watch the cognitive-memory graph forming:

```text
$ simard memory stats        # daemon stopped → direct on-disk open

  ... per-type table ...

edges / connections:
  DERIVES_FROM                 12     (fact -> episode)
  PROCEDURE_DERIVES_FROM        3     (procedure -> episode)
  SIMILAR_TO                    0     (fact <-> fact)
  SUPERSEDES                    0     (deduped snapshot)
  facts with provenance:  4 / 5
  snapshot dedup:         1 distinct caller keys / 6 snapshot facts
```

| Line | Meaning |
|---|---|
| `DERIVES_FROM` | Provenance edges from distilled facts back to their source episodes (the read side of `store_fact_with_provenance`). |
| `PROCEDURE_DERIVES_FROM` | Provenance edges from procedures back to the episodes they were distilled from. |
| `SIMILAR_TO` | Fact↔fact similarity edges. |
| `SUPERSEDES` | Edges left by caller-key dedup (new snapshot → archived prior). |
| `facts with provenance: X / Y` | `X` of `Y` facts carry at least one `DERIVES_FROM` edge. |
| `snapshot dedup: D / T` | `T` snapshot facts (live + superseded revisions) collapsed onto `D` distinct caller keys. `T` well above `D` is the visible dedup signal. Scoped to the `goal-board:snapshot` concept only — the per-goal `goal-store:record:{slug}` dedup family (above) is not counted here. |

The `--json` output mirrors this under stable keys: an `edges` object
(`derives_from`, `procedure_derives_from`, `similar_to`, `supersedes`), a
`provenance` object (`facts_with_provenance`, `facts_total`), and a
`snapshot_dedup` object (`distinct_caller_keys`, `snapshot_facts`).

**Limitations.** The pinned `amplihack-memory` rev exposes provenance readers
but **no public per-type edge counter**, so `SIMILAR_TO` and `SUPERSEDES` are
reported as `0`; the `snapshot dedup` line is the computed proxy for
`SUPERSEDES` activity. The edge counts also require reading the graph directly,
which the daemon's memory **socket does not expose** — when the daemon is up the
section prints `(edges: run with daemon stopped for graph stats)`. Stop the
daemon (or point `stats` at an idle state-root) for the real counts. The whole
section is read-only and never fails the report.

The `episodic` count here is the number of episodes **stored**; it is
distinct from the `… episodes` figure in the per-cycle OODA log, which
counts episodes **recalled for the current objective** (keyword-relevant,
self-session noise filtered). A populated store can legitimately recall
`0` episodes for an unrelated objective. See
[Memory introspection CLI](reference/simard-memory-cli.md) for the full
contract, including the type→field mapping and the stored-vs-recalled
distinction.

## Cross-session recall

Semantic, procedural, and prospective memory survive process restarts and are queried at the start of every engineer dispatch. When the daemon spawns a new engineer for a goal it seeds the engineer's working memory with the most relevant prior episodes for that goal-id, so engineers continue where the previous attempt left off.

**Durability guarantee.** The library backend (`state_root/cognitive`) makes every acknowledged write durable on its own: each store operation issues a per-write `fsync` barrier into the write-ahead log, so a write that returned `Ok` survives even an *un-checkpointed* crash. A graceful `checkpoint()` (which the OODA loop runs at consolidation) folds the WAL into the main database file; a subsequent clean reopen then needs no replay. If a process is killed mid-write, the next `LibraryCognitiveMemory::open` routes through the library's `open_with_recovery` ladder (corrupt-WAL tail quarantine + good-prefix replay, and corrupt-catalog quarantine as a last resort — memory-lib #92–#97), so a later session never crash-loops on a damaged store.

Because the store persists at a per-`state_root` path with no shared global state, recall is *cross-process*, not just cross-handle: a `simard` process started later (or a separate operator reading via `simard memory stats`) opens the same on-disk store through the tier-2 "direct open" path and observes every committed write — counts, the provenance / dedup graph edges, and literal fact content. This contract is gated end-to-end by `tests/cognitive_memory_cross_session_recall.rs` (driven by `tests/gadugi/cross-session-recall.yaml`): Session A writes through `LibraryCognitiveMemory` and a **separate real `simard` process** recalls via `simard memory stats --json` / `simard memory dump --type=facts --json`, including a crash-recovery step that reopens an un-checkpointed store.

## On-disk layout

The library backend persists at `state_root/cognitive` (a LadybugDB `GraphStore`). In production `state_root` is `~/.simard`:

```
~/.simard/
  └── cognitive/             # library CognitiveMemory store (LadybugDB):
                             #   sensory, working, episodic, semantic,
                             #   procedural, prospective
```

The library owns its own durability (WAL + CHECKPOINT). The old native store at `~/.simard/cognitive_memory.ladybug` is abandoned by Phase 2b — it is never read or migrated, and the memory store rebuilds from scratch in `cognitive/`.

Inspect with `simard memory stats` / `simard memory dump` (see
[Memory introspection CLI](reference/simard-memory-cli.md)), or with the
dashboard's **Memory** tab ([Dashboard](dashboard.md)) — the graph view
supports per-type filters and full-text search across the persistent
layers.

![Memory tab](assets/dashboard-memory.png)

## Hive event bus (multi-agent knowledge sharing)

When multiple agents (engineer subprocesses, meeting facilitators, gym runs) operate concurrently, they share knowledge through the **hive event bus** (`src/hive_event_bus.rs`). Each agent emits memory events that other agents can subscribe to, enabling cross-agent learning without a central coordinator.

For multi-host coordination see [Distributed operations](distributed-operations.md).

## Code entry points

- `src/cognitive_memory/mod.rs` — `CognitiveMemoryOps` trait + DTOs
- `src/cognitive_memory/library_adapter.rs` — `LibraryCognitiveMemory` (the sole backend)
- `src/hive_event_bus.rs` — multi-agent event bus

## Related

- [Cognitive Memory Architecture](architecture/cognitive-memory.md) (canonical, full detail)
- [Episode ingestion policy & automatic promotion](architecture/episode-ingestion-policy.md) — the classifier that keeps episodic memory clean and the scheduler that promotes it automatically (#2327)
- [Episode ingestion classifier API](reference/episode-ingestion-classifier.md) — `classify`, `sanitize_transcript`, the metadata taxonomy, and the intake wiring (#2327)
- [Automatic distillation scheduler API](reference/automatic-distillation-scheduler.md) — `run_scheduled_distillation`, the `distill_trigger` predicate, config fields, and the procedures extension (#2327)
- [Distill recipe output capture](reference/distill-recipe-output-capture.md) — capturing the distill agent's `{ "facts": …, "procedures": … }` JSON via `recipe-runner-rs --output-format json`, the envelope parser, and the recipe-asset sync (#2401)
- [Configure episode hygiene and promotion](howto/configure-episode-hygiene-and-promotion.md) — operator tuning and observability (#2327)
- [Library-backed Cognitive Memory](architecture/cognitive-memory-library-adapter.md) — the `amplihack-memory-lib` backend, now the sole on-disk store (de-fork Phase 2b)
- [Memory introspection CLI](reference/simard-memory-cli.md) — `simard memory stats` / `simard memory dump` for read-only, lock-safe per-type counts and sample rows
- [OODA procedural memory](reference/ooda-procedural-memory.md) — how successful OODA outcomes become procedures
- [Procedural-memory store idempotency](reference/cognitive-memory-procedural-idempotency.md) — exact-name dedup that stops repeated cycles re-storing identical procedures (#2298)
- [Goal–prospective memory mirror](reference/goal-prospective-memory-mirror.md) — how Active goals become prospective triggers
- [Goal-board prospective reconcile](reference/goal-board-prospective-reconcile.md) — the per-cycle board-sourced mirror the live daemon runs so triggers actually populate (#2308)
- [Prospective-trigger firing](reference/prospective-trigger-firing.md) — how the OODA objective probe and case-insensitive match make stored triggers fire
- [Episodic keyword recall](reference/cognitive-memory-episodic-recall.md) — how stored episodes surface for a matching objective
- [Cognitive-memory provenance](reference/cognitive-memory-provenance.md) — DERIVES_FROM / PROCEDURE_DERIVES_FROM edges linking distilled facts and procedures back to their source episodes (#2325)
- [Phase-weighted ranked fact recall & snapshot retention](reference/cognitive-memory-ranked-recall.md) — multi-signal ranked recall with per-OODA-phase weights, plus CallerKey dedup/SUPERSEDES and pruning for snapshot/goal records (#2329)
- [Ranked episodic recall & memory reinforcement](reference/cognitive-memory-ranked-episodic-recall.md) — extends ranked recall to episodes (with a UNION backfill that keeps compressed consolidation sources recallable) and adds a usage/recency reinforcement seam plus `CognitiveFact` observability, recording accesses at the point recalled memories are surfaced into a cycle (per-action attribution is a future refinement) (#2395)
- [Dashboard](dashboard.md) — Memory tab
- [Daemon mode](daemon-mode.md) — when consolidation runs
