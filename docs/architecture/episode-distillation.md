---
title: Episode distillation
description: How Simard periodically distills batches of recent episodes into semantic facts using an LLM-backed recipe, what concept labels are produced, when the distillation pass fires, and how distilled episodes are marked to prevent reprocessing.
last_updated: 2026-06-14
owner: simard
doc_type: concept
related:
  - ./distillation-semantic-handoff.md
  - ../reference/simard-memory-remember-cli.md
  - ../reference/distill-write-boundary-gate.md
  - ./cognitive-memory.md
  - ./episode-ingestion-policy.md
  - ../reference/cognitive-memory-preparation-filters.md
  - ../reference/cognitive-memory-episodic-recall.md
  - ../reference/ooda-procedural-memory.md
  - ../reference/cognitive-memory-procedural-idempotency.md
  - ../reference/cognitive-memory-provenance.md
  - ../reference/automatic-distillation-scheduler.md
  - ../reference/distill-recipe-output-capture.md
  - ../memory.md
---

# Episode distillation

> Shipped in issue [#2281](https://github.com/rysweet/Simard/issues/2281)
> as PR-B (episode distillation). Builds on PR-A (preparation filters)
> and feeds PR-C (episodic recall) with a higher-quality semantic store.

> **Superseded result path (#2679).** Everything below about *how distilled
> facts reach memory* — the agent writing a `{ "facts": [...] }` envelope to a
> `facts_output_path` file, Simard reading that file and deserializing it with
> `serde_json`, the `ParseFailure` / `parse_fail` class, and the
> raw-capture-on-parse-failure diagnostic — has been **replaced** by a semantic
> agent-to-agent handoff. The distiller now writes each fact **directly** into
> cognitive memory via [`simard memory remember`](../reference/simard-memory-remember-cli.md);
> there is no returned document and **no parse**, so a trailing comma or a noisy
> launcher banner can no longer discard a batch. The batch selection, concept
> labels, threshold gate, and mark-everything rule described below are
> unchanged. See
> [Distillation semantic handoff](./distillation-semantic-handoff.md) for the
> current result path and
> [Distill write-boundary gate](../reference/distill-write-boundary-gate.md) for
> where the reliability/quarantine/dedup gate now lives.

Episode distillation is the periodic process that scans recent
**episodic** memory and extracts **semantic facts** from it using a
deterministic LLM recipe. It is the missing half of memory
consolidation: prior to PR-B, `consolidate_episodes` only performed
textual dedup, which left the consolidation compression ratio at 0%
every cycle because raw episodes rarely have identical text. PR-B
adds true semantic distillation alongside the existing textual pass.

The pass runs inside the `ConsolidateMemory` action handler — the
same action the `__memory__` synthetic priority dispatches to. No new
synthetic priority is added. The existing deterministic-brain routing
locked in by issue [#2286](https://github.com/rysweet/Simard/issues/2286)
is preserved.

> **Now also automatic (#2327).** Since
> [#2327](https://github.com/rysweet/Simard/issues/2327), distillation no
> longer waits for the brain to choose `ConsolidateMemory`: an
> [automatic promotion scheduler](./episode-ingestion-policy.md) also fires
> this pass at the end of every OODA cycle once the undistilled backlog
> reaches a threshold or a cycle-count interval elapses. The pass was also
> extended to emit **procedures** (not just facts), both written with
> provenance. See the
> [automatic distillation scheduler reference](../reference/automatic-distillation-scheduler.md).
> The fact pipeline described below is unchanged.

---

## Why distillation matters

Semantic memory is the layer the OODA brain reads first during
preparation. Facts are higher signal than raw episodes because:

- Each fact has a confidence score, a concept label, and source-id
  provenance — facts can be ranked, filtered, and traced.
- Facts are short and substring-searchable, so `search_facts` returns
  meaningfully diverse results within the prepared-context budget.
- Facts accumulate across runs even after episodes are pruned.

Without distillation, the only writers to semantic memory were
`goal-store:record` (goal mirror) and rare manual stores. The bulk of
operational knowledge — what worked, what broke, what was tried —
sat in episodic memory where it was hard to retrieve and easy to lose
during consolidation. PR-B closes that gap.

---

## Pipeline overview

```
Episodic memory (newest first)
  │
  │  list_undistilled_episodes(50)
  ▼
Batch of up to 50 episodes where distilled = 0
  │
  ├─ if batch.len() < 20  →  skip pass, no LLM call, no markers
  │
  ▼
Serialize as JSON, invoke recipe-runner-rs (agent writes facts_output_path file)
  │
  ▼
prompt_assets/simard/recipes/distill-episodes.yaml
  │  classifies each episode into:
  │    - pr-pattern        (PR-shaped events, merge sequences, review patterns)
  │    - bug-pattern       (bug reproductions, root causes, recurring failures)
  │    - lesson-learned    (decisions, tradeoffs, things that surprised the engineer)
  │    - skip              (truly low-signal — startup logs, retries, etc.)
  ▼
Read the dedicated facts file the agent WROTE (never stdout):
  { "facts": [ { concept, content, source_episode_id }, ... ], "procedures": [ ... ] }
  │
  ▼
For each fact:
    store_fact_with_provenance(
        concept, content, confidence=0.7,
        source_id=format!("distill:{source_episode_id}"),   // textual id retained
        tags=Some(&[concept]), metadata=None,
        source_episode_ids=&[source_episode_id])             // DERIVES_FROM edge (#2325)
  │
  ▼
For EVERY input episode (even those classified "skip"):
    mark_episode_distilled(node_id)
```

Each distilled fact now also gets a `DERIVES_FROM` graph edge back to its
source episode, in addition to the textual `distill:{id}` `source_id`
which is retained for backward compatibility. See
[Cognitive-memory provenance](../reference/cognitive-memory-provenance.md).

The mark-everything rule prevents prompt-replay loops: an episode
classified "skip" once will not be re-fed to the LLM on the next
pass. If the entire batch errors, *no* markers are set — the batch
retries on the next pass.

---

## When the pass fires

The pass runs inside `dispatch_consolidate_memory`
(`src/ooda_actions/simple_actions.rs`), alongside the existing
textual `consolidate_episodes(20)` call. Both run on every
`ConsolidateMemory` action; failure of one does not abort the other.

`ConsolidateMemory` is dispatched when the OODA brain emits a
`__memory__` synthetic priority, which the priority router maps to
the `ConsolidateMemory` action kind. The brain emits `__memory__`
opportunistically — typically once every several cycles, governed by
the existing memory-pressure heuristics.

### Threshold gate

Even when the action fires, the distillation pass itself is gated by
a minimum batch size:

| Constant                  | Default | Source file                                   |
|---------------------------|---------|-----------------------------------------------|
| `DISTILL_BATCH_SIZE`      | 50      | `src/memory_consolidation/distillation.rs`    |
| `DISTILL_MIN_EPISODES`    | 20      | `src/memory_consolidation/distillation.rs`    |

If `list_undistilled_episodes(DISTILL_BATCH_SIZE)` returns fewer than
`DISTILL_MIN_EPISODES` rows, the pass is **skipped entirely**:

- No recipe is invoked.
- No `store_fact` is called.
- No `mark_episode_distilled` is called.

This is intentional. Distillation is a many-to-few operation; running
it on 3 episodes wastes an LLM call for little quality gain. The
batch waits for the next pass.

---

## The recipe

`prompt_assets/simard/recipes/distill-episodes.yaml` is a one-step
recipe with a single LLM agent. It follows the same shape as
`recipe_merge_judge` and `recipe_progress_checker`:

- **Input context variable**: `episodes` — JSON array of objects with
  `{ id, source_label, temporal_index, content }`. The
  `temporal_index` is the monotonic `i64` clock that ships on every
  `CognitiveEpisode` row; it is **not** a wall-clock timestamp. The
  recipe only needs ordering, not human-readable time, so no
  `chrono::DateTime` conversion is performed at the boundary.
- **Prompt**: instructs the agent to classify each episode into one
  of the three concept labels (or `skip`) and **write** a JSON object
  `{ "facts": [ { "concept": "...", "content": "...",
  "source_episode_id": "..." } ], "procedures": [ ... ] }` to a
  dedicated facts file.
- **Output**: the agent WRITES its JSON envelope to a dedicated
  per-invocation facts file whose absolute path is passed to the recipe
  as `-c facts_output_path=<tmp>` and interpolated into the prompt as
  `{{facts_output_path}}`. After the runner exits, the Rust caller reads
  **that file** and deserializes it — stdout is never the result channel,
  so the copilot launcher banner and log lines can no longer contaminate
  the parse (issues [#2622](https://github.com/rysweet/Simard/issues/2622)
  / [#2619](https://github.com/rysweet/Simard/issues/2619)). A missing,
  empty, or unparseable facts file — or a failed run — causes the caller
  to return `Err` (which then triggers the "no markers set" retry
  behaviour above); there is **no** stdout fallback (a silent fallback is a
  silent failure). See
  [Distill recipe output capture](../reference/distill-recipe-output-capture.md)
  for the file-channel contract, the parser, and failure semantics.

The Rust-side invocation shells out to `recipe-runner-rs` with an
argv-vector (no shell): the recipe path as a positional arg, the episodes
batch inlined as a single `-c episodes=<json>` context entry, and the
facts file path as `-c facts_output_path=<tmp>`, with
`AMPLIHACK_AGENT_BINARY` in the environment. `--output-format json` is
still passed so a runner-level failure surfaces a structured error on
stdout for the terminal-failure message, but the distill **result** is
read from the facts file, never stdout — the file channel is what closes
the latent silent no-op that
[#2401](https://github.com/rysweet/Simard/issues/2401) first fixed via
stdout capture and that [#2622](https://github.com/rysweet/Simard/issues/2622)
/ [#2619](https://github.com/rysweet/Simard/issues/2619) hardened against
launcher-banner contamination. The sibling
`stewardship::recipe_merge_judge::RecipeMergeJudge` and
`goal_curation::recipe_progress_checker::RecipeProgressChecker` still parse
the runner's text/stdout output and are intentionally left unchanged (they
do not carry the distill agent's large structured payload).

The recipe is loaded with the same resolution order Simard uses
elsewhere:

1. `~/.simard/prompt_assets/simard/recipes/distill-episodes.yaml`
   (user override)
2. `<repo>/prompt_assets/simard/recipes/distill-episodes.yaml`
   (in-tree default)

### Concept labels

The recipe is constrained to exactly three labels:

| Label             | Use                                                                   |
|-------------------|-----------------------------------------------------------------------|
| `pr-pattern`      | Pull-request shaped events: merge sequences, CI patterns, PR scope    |
| `bug-pattern`     | Bug reproductions, root causes, recurring failure modes               |
| `lesson-learned`  | Decisions made, tradeoffs encountered, engineer-surprising outcomes   |

A fourth pseudo-label `skip` is allowed in the agent's reasoning but
must not appear in the `facts` array — skipped episodes simply
contribute no fact.

The label set is deliberately small to keep search-time signal
useful. Adding labels expands the search surface without immediately
improving fact relevance; new labels should be added only when a
clear retrieval pattern motivates them.

#### Surface-form canonicalization

The prompt constrains the label to those three strings, but an LLM
routinely varies the *surface form* of a label it clearly intends —
title/upper case (`PR-Pattern`, `BUG-PATTERN`), surrounding whitespace
or quotes/sentence punctuation (`" bug-pattern "`, `pr-pattern.`), and
`_`/space separators (`pr_pattern`, `lesson learned`). The shared
reliability scorer's [`fact_reliability::canonical_concept`](../reference/distill-write-boundary-gate.md)
folds these variants back to the canonical lower-hyphen label **before**
the closed-set match, so a well-formed label is not mistaken for
off-spec on cosmetics. Normalization is limited to case-folding,
trimming, and `_`/space→`-` (with repeated hyphens collapsed); a concept
that does not normalize to exactly one of the three labels —
`made-up-label`, `pr-patterns`, `pull-request` — canonicalizes to
`None`.

Post-#2679, concept validity is a **reliability nudge, not a drop
filter**: the write-boundary gate scores canonical-concept membership as
`+0.1` on the `[0,1]` reliability score (see the
[write-boundary gate](./distillation-semantic-handoff.md)), it does not
gate promotion. A grounded, non-empty fact clears the threshold with or
without a known concept, so canonicalization *recovers the nudge* for a
surface-form variant the model clearly intended rather than salvaging a
fact from being dropped. Recovering the canonical label also keeps the
concept column uniform for downstream dedup and recall regardless of how
the model spelled it. See the [fact-yield metric](#fact-yield) for how the
runtime yield of the whole path is tracked over time.

---

## Trait surface

The pass uses two `CognitiveMemoryOps` methods
(`src/cognitive_memory/mod.rs`), both with default no-op
implementations so any backend that lacks a distilled-flag API keeps
compiling:

```rust
pub trait CognitiveMemoryOps {
    // ... existing methods ...

    /// Mark an episode as distilled so subsequent distillation passes skip it.
    /// Default impl is a no-op for backends that do not support metadata mutation.
    fn mark_episode_distilled(&self, node_id: &str) -> SimardResult<()> {
        Ok(())
    }

    /// Return up to `limit` undistilled episodes, newest first.
    /// Default impl returns empty, which makes the distillation pass a no-op
    /// for backends that do not track the `distilled` flag.
    fn list_undistilled_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        Ok(vec![])
    }
}
```

`LibraryCognitiveMemory` (the sole backend) **overrides both** by
delegating to the `amplihack-memory-lib` `CognitiveMemory`, which
exposes `mark_episode_distilled(node_id) -> bool` and
`list_undistilled_episodes(limit) -> Vec<EpisodicMemory>` directly:

```rust
fn mark_episode_distilled(&self, node_id: &str) -> SimardResult<()> {
    self.lock()?.mark_episode_distilled(node_id); // bool ignored (id-missing latch)
    Ok(())
}

fn list_undistilled_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
    Ok(self.lock()?
        .list_undistilled_episodes(limit as usize)
        .into_iter()
        .map(to_episode)
        .collect())
}
```

> **De-fork note (Phase 2b).** The native fork once owned these
> implementations against an lbug-backed `Episode` schema. With the fork
> deleted, the `distilled` flag and its persistence live in the library;
> the Simard adapter simply forwards to it. (During Phase 2a the library
> backend had no distilled-flag API and degraded these to a loud no-op —
> that gap is closed.)

### Schema

The library's `Episode` node carries a `distilled` flag:

| Column      | Meaning                                                |
|-------------|--------------------------------------------------------|
| `distilled` | set once an episode has been processed by distillation |

The library owns the column and its migration. Legacy/un-flagged rows
read as undistilled, so the first post-upgrade pass naturally processes
everything; no offline migration step is required.

### `compressed` vs `distilled` are independent

The episode schema already has a `compressed: bool` column written by
the textual `consolidate_episodes` pass. PR-B's `distilled: i64` is
**independent** of it:

- `consolidate_episodes` (textual dedup) writes `compressed`.
- `distill_recent_episodes` (semantic many-to-few) writes `distilled`.
- An episode can land in any of the four states `(compressed,
  distilled) ∈ {(0,0), (0,1), (1,0), (1,1)}` depending on which
  passes have processed it.

Neither flag implies the other. The two passes co-exist inside
`ConsolidateMemory` and are emitted on separate log lines so an
operator can attribute work to the correct pass.

---

## Public functions

### `distill_recent_episodes`

```rust
pub fn distill_recent_episodes(
    memory: &dyn CognitiveMemoryOps,
    repo_root: &Path,
) -> SimardResult<DistillReport>;
```

Runs one distillation pass. Returns a `DistillReport` describing what
happened.

### `DistillReport`

```rust
pub struct DistillReport {
    /// Number of undistilled episodes pulled from the store.
    pub input_count: u32,
    /// Number of facts emitted by the recipe.
    pub fact_count: u32,
    /// Number of procedures emitted by the recipe.
    pub procedure_count: u32,
    /// Number of episodes marked distilled after the pass.
    pub marked_count: u32,
    /// Number of candidate facts blocked by the reliability gate (issue #2433).
    pub quarantined_count: u32,
}
```

`fact_count + quarantined_count` is the total number of candidate facts the
recipe emitted for the pass; `quarantined_count` counts the candidates the
ISAO reliability gate either quarantined (low score) or refused to promote
because a stronger prior already existed on the concept.

## Reliability gate (issue #2433)

Before a distilled fact is promoted into semantic memory it is **self-assessed**
and gated on `Fact.confidence` — turning the formerly-constant `0.7` into a
live, computed signal (BGML's *information self-assessment ownership*, ISAO).

`fact_reliability::score_fact_reliability(concept, content, grounded) -> f64`
scores each candidate in `[0.0, 1.0]` from cheap local signals (no extra LLM
call). It is a **pure per-fact** function shared by both write-boundary seams
(the daemon IPC gate and the in-process distill sink), so a fact scores
identically no matter which boundary writes it:

| Signal | Weight | Meaning |
|--------|--------|---------|
| Provenance grounding | 0.5 | `source_episode_id` (trimmed via `fact_reliability::normalize_source_episode_id`) is one of the episodes fed to the recipe this pass (not hallucinated). Both seams normalize the cited id the same way, so an id an LLM re-emitted with stray surrounding whitespace still grounds (and threads a resolvable `DERIVES_FROM` edge) instead of being silently quarantined. **Necessary**: without grounding a fact tops out at 0.4 and is always quarantined. |
| Content quality | ≤0.3 | Empty / whitespace-only content is a **hard gate** (score `0.0`); otherwise ≥3 words earns the full 0.3, 1–2 words a partial 0.15. |
| Concept validity | 0.1 | Concept canonicalizes to one of `pr-pattern` / `bug-pattern` / `lesson-learned`. A **nudge, not a gate**. |

> The legacy batch scorer's **corroboration** term was **dropped in
> [#2679](https://github.com/rysweet/Simard/issues/2679)**: a per-fact IPC call
> has no batch to corroborate against, and the term was disposition-neutral (it
> only nudged an already-storable `0.9 → 1.0`, never flipped
> store↔quarantine), so both seams agree on every decision without it.

A candidate scoring below `DISTILL_RELIABILITY_THRESHOLD` (0.5) is **quarantined**
— not written. Because grounding (0.5) is necessary to reach the threshold, a
hallucinated-provenance fact scores at most `0.4` and an empty fact scores `0.0`;
both are quarantined. A nominal grounded, known-concept, ≥3-word fact scores `0.9`
— at or above the legacy baseline — so good facts keep their prior behaviour.

A surviving candidate is written with its *computed* confidence, but never
**downgrades** a stronger existing copy of the **same fact**. The don't-clobber
guard matches on fact *identity* (concept **and** content), not the concept
label alone: the recipe emits only three concept labels and every good fact
scores identically, so a concept-only guard would quarantine every distinct fact
after the first one stored under a label and silently neuter distillation.
Identity matching blocks only a genuine re-distillation of the same content at a
lower-or-equal confidence, while distinct lessons that share a label accumulate
(`search_facts(concept, _, score)` is consulted, then filtered on content).

Each pass records a `distill_reliability_gate` metric whose value is the
block-rate (`quarantined / candidate_facts`), with the counts in the context
payload, so the gate's effect is measurable before/after from `metrics.jsonl`.

### Reduction ratio

The textual `consolidate_episodes` step reports a **compression
ratio** (input bytes vs. output bytes after dedup), which sits at 0%
in unmodified runs because raw episodes rarely share identical text.
Distillation reports a different metric — a **reduction ratio** —
because the operation is many-to-few semantic extraction, not
textual deduplication:

```
distill: 25 episodes → 3 facts, 25 marked (reduction 88%)
```

Where `reduction = 1 - (fact_count / input_count)`. The two ratios
measure different things and must not be summed or compared
directly; they are emitted on separate log lines so an operator can
tell which pass — textual or semantic — is doing the work on a given
cycle.

---

## Fact-yield

Fact-yield is the number of facts a pass promotes per unit of
consolidation input (facts-per-episode-batch):

```
distill_fact_yield = fact_count / input_count      per completed pass
```

Because the LLM recipe call is non-deterministic, real end-to-end
fact-yield is a *runtime* property, not a fixed number. It is therefore
made **observable and trendable** as a durable self-metric rather than
asserted by a static benchmark: every **completed** pass records one
`distill_fact_yield` event to `metrics.jsonl` via
`self_metrics::record_metric` (see [Observability](#distill_fact_yield-fact-yield-metric)),
so a regression in the distiller's yield shows up as a falling mean the
same way `recall_precision_at_k` surfaces ranked-recall regressions. The
metric `value` is the ratio above; its context carries
`{input_count, fact_count, quarantined, fact_yield}` so a low yield can be
attributed to gate blocks versus a low-signal batch.

> **Retired in [#2679](https://github.com/rysweet/Simard/issues/2679):**
> the former *deterministic* fact-yield regression benchmark
> (`distillation_fact_yield_bench.rs`) measured the yield of the
> `parse_facts_document` + `assess_fact_reliability` parse/filter path,
> which no longer exists — the distiller now writes each fact directly
> through the [write-boundary gate](./distillation-semantic-handoff.md).
> On that path concept canonicalization is a disposition-neutral
> reliability *nudge* (`+0.1`, [`fact_reliability::score_fact_reliability`](../reference/distill-write-boundary-gate.md)),
> **not** a promotion gate, so it no longer moves gate fact-yield and the
> benchmark's before/after premise (canonicalization recovering dropped
> surface-form variants) no longer holds. Fact-yield is now tracked by the
> runtime `distill_fact_yield` series above.

---

## Observability

The pass emits two log lines:

```
[simard] distill: 25 episodes pulled (batch size 50, min 20)
[simard] distill: 25 episodes → 3 facts, 25 marked
```

When skipped:

```
[simard] distill: 10 episodes pulled, below min 20, skipped
```

When the recipe errors:

```
[simard] distill: 25 episodes pulled, recipe error: <message>, no markers set, retry next pass
```

These are low-cardinality `tracing::info!` lines suitable for
grep-based monitoring.

### `distill_success_rate` (reliability metric)

> Added in issue [#2461](https://github.com/rysweet/Simard/issues/2461).

A distillation failure is **non-fatal**: `dispatch_consolidate_memory`
folds the `Err` into a human-readable string and still reports the
`ConsolidateMemory` action successful. Before #2461 the only trace was the
`tracing::warn!` line above, so the *frequency* of failures — and therefore
the silent degradation of semantic recall — was invisible.

Every pass that **ran the recipe** (success OR a recipe/parse failure;
below-threshold skips are excluded) now records a `distill_success_rate`
metric event to `metrics.jsonl` via `self_metrics::record_metric`, mirroring
`distill_reliability_gate`. The metric scope is the **recipe + output-parse
stage** — exactly the #2461 failure surface; downstream memory-write failures
are a separate subsystem (they propagate as `Err` and are not folded into this
metric). The metric `value` is `1.0` on success and `0.0`
on failure, so the mean over passes is the success rate; the context payload
carries `{outcome, recipe_exited_ok, parse_attempted, parse_success,
failure_class, input_count, fact_count}`:

```
distill_success_rate       = mean(value)                          over ran passes
distill_parse_success_rate = mean(value)                          over passes that reached parsing
```

`distill_parse_success_rate` is emitted as a **first-class metric**
(issue [#2512](https://github.com/rysweet/Simard/issues/2512)) for the subset
of passes that actually reached output parsing (`parse_attempted == true`), so
its plain mean is exactly successes-vs-attempts — no post-hoc filtering of
`distill_success_rate` events is needed (the older `parse_attempted` /
`parse_success` context flags remain for back-compatible derivation). It
isolates the "recipe exited 0 but the agent's facts document was
missing/empty/unparseable" mode (`failure_class = parse-failure`, issues
#2622/#2619) from the "recipe process exited non-zero" mode
(`copilot-terminal-failure`), which never reached parsing and emits **no**
parse-rate event. This is the rate the file-channel fix (#2622/#2619) drives
toward `1.0`. Because the data lives in `metrics.jsonl` (operator
runtime state, queryable via `self_metrics::query_metrics`), the rates are
computed over a rolling window — no point-in-time findings doc is committed.

The companion robustness fix is parser-side: `scan_for_facts_object` now
returns the facts from the **last non-empty** balanced `{...}` object that
parses (not the first), with string-aware brace scanning, so a leading
banner/thinking object no longer shadows the agent's facts object and an
accidental trailing empty object never discards earlier facts (the t=7517
parse-failure mode; see "The recipe" above).

### `distill_fact_yield` (fact-yield metric)

> Perpetual-cognition goal: make distillation fact-yield observable.

Fact-yield — promoted facts per input episode — was previously visible only
as inert `input_count` / `fact_count` context on `distill_success_rate`,
which is a **binary** success signal; its mean is the pass success rate, not
the yield. To make yield itself trendable, every **completed** pass now also
records one `distill_fact_yield` event to `metrics.jsonl` via
`self_metrics::record_metric`, mirroring `distill_reliability_gate` and the
ranked-recall `recall_precision_at_k` series.

```
distill_fact_yield = fact_count / input_count      per completed pass
```

The metric `value` is the ratio (`0.0` when a pass pulled no episodes — the
`DISTILL_MIN_EPISODES` skip makes that unreachable on the emitting path, but
the helper is total). The context payload carries
`{input_count, fact_count, quarantined, fact_yield}`, so a consumer can
recompute the ratio, segment by pass size, or attribute a low yield to gate
blocks (`quarantined`) versus a low-signal batch. Because the data lives in
`metrics.jsonl` (operator runtime state, queryable via
`self_metrics::query_metrics`), the yield mean is computed over a rolling
window — a regression shows up as a falling mean rather than a silent
degradation of semantic-memory growth. Emitted only on a completed pass
(skips and recipe errors emit no yield event), so the series carries signal
only. Best-effort and `cfg!(test)`-gated so unit tests never append to the
operator's real `metrics.jsonl`.

---

## Examples

### Example 1 — typical pass

Episodic memory has 30 undistilled episodes after a busy
goal-curation phase:

```
1. dispatch_consolidate_memory runs.
2. consolidate_episodes(20) does textual dedup (existing behaviour).
3. distill_recent_episodes:
     - list_undistilled_episodes(50) returns 30 episodes
     - 30 ≥ 20 → proceed
     - recipe emits 4 facts: 2 pr-pattern, 1 bug-pattern, 1 lesson-learned
     - store_fact called 4 times
     - mark_episode_distilled called 30 times
     - returns DistillReport { input: 30, fact: 4, marked: 30 }
```

Log:

```
[simard] distill: 30 episodes pulled (batch size 50, min 20)
[simard] distill: 30 episodes → 4 facts, 30 marked
```

### Example 2 — under threshold

Only 8 undistilled episodes since last pass:

```
distill: 8 episodes pulled, below min 20, skipped
```

`DistillReport::skipped()` is returned; the textual pass still runs.

### Example 3 — recipe error

The runner exits non-zero, or the agent's dedicated facts file is
missing, empty, or carries no parseable facts object:

```
distill: 40 episodes pulled, recipe error: <message>, no markers set, retry next pass
```

`mark_episode_distilled` is **not** called. The same 40 episodes are
eligible on the next pass. See
[Distill recipe output capture](../reference/distill-recipe-output-capture.md#failure-semantics)
for the exact failure matrix.

---

## Tuning the constants

Defaults reflect a balance between LLM cost and freshness:

| Constant                | Default | Lower bound | Upper bound | Effect of change                                  |
|-------------------------|---------|-------------|-------------|---------------------------------------------------|
| `DISTILL_BATCH_SIZE`    | 50      | 10          | 200         | Higher → fewer LLM calls, larger prompts          |
| `DISTILL_MIN_EPISODES`  | 20      | 5           | `BATCH_SIZE` | Higher → less frequent passes, more amortization |

To tune at build time, edit the constants in
`src/memory_consolidation/distillation.rs`. There is no runtime
configuration knob in PR-B; operational experience may motivate a
config-file or env-var override in a future PR.

---

## Interaction with other memory layers

| Layer              | Effect of PR-B                                                                  |
|--------------------|---------------------------------------------------------------------------------|
| Episodic memory    | New `distilled` column. Episodes are **not** deleted; only marked.              |
| Semantic memory    | New facts arrive with concepts `pr-pattern`, `bug-pattern`, `lesson-learned`.   |
| Procedural memory  | Untouched. (PR-C touches procedural memory.)                                     |
| Prospective memory | Untouched.                                                                       |
| Working memory     | Untouched.                                                                       |
| Sensory memory     | Untouched.                                                                       |
| Preparation        | New fact concepts are searchable by `preparation_memory_operations`. PR-A's     |
|                    | filters do **not** touch the three new concepts, so they flow through normally. |
| Episodic recall    | PR-C's episodic recall benefits from distilled summaries (better substring hits).|

---

## Code location

| Item                                | File                                                   |
|-------------------------------------|--------------------------------------------------------|
| `distill_recent_episodes`           | `src/memory_consolidation/distillation.rs`             |
| `DistillReport`                     | `src/memory_consolidation/distillation.rs`             |
| `DISTILL_BATCH_SIZE` / `DISTILL_MIN_EPISODES` | `src/memory_consolidation/distillation.rs`   |
| Recipe                              | `prompt_assets/simard/recipes/distill-episodes.yaml`   |
| Dispatcher hook                     | `src/ooda_actions/simple_actions.rs` (`dispatch_consolidate_memory`) |
| Trait methods                       | `src/cognitive_memory/mod.rs`                          |
| Adapter impls (delegation)          | `src/cognitive_memory/library_adapter.rs`              |
| Episode schema + `distilled` flag   | `amplihack-memory-lib` (`CognitiveMemory`)             |
| Tests                               | `src/memory_consolidation/distillation_tests.rs`,      |
|                                     | `src/cognitive_memory/tests_library_parity.rs` (round-trip) |

---

## Testing

### Trait round-trip tests

In `src/cognitive_memory/tests_library_parity.rs` (against the library backend):

| Test                                              | Coverage                                                    |
|---------------------------------------------------|-------------------------------------------------------------|
| `list_undistilled_episodes_returns_newest_first`  | Ordering: newest first. Episode ids are time-prefixed, so the library returns newest-first without consulting `temporal_index`. |
| `mark_episode_distilled_round_trips`              | `mark` then `list` excludes the marked row                  |
| `list_undistilled_respects_limit`                 | `limit` parameter honoured                                  |

### Distillation pass tests

In `src/memory_consolidation/distillation_tests.rs`:

| Test                                                          | Coverage                                              |
|---------------------------------------------------------------|-------------------------------------------------------|
| `distillation_skipped_under_min_threshold`                    | 10 episodes < 20 → no LLM call, no markers            |
| `distillation_stores_facts_and_marks_originals`               | 25 episodes → 3 facts stored, **25** markers set       |
| `distillation_handles_recipe_error_without_marking`           | recipe Err → no facts, no markers, retry on next pass  |
| `distillation_marks_episodes_classified_as_skip`              | 5 episodes, 0 facts from recipe, still 5 markers       |
| `distillation_does_not_touch_compressed_flag`                 | Asserts the `compressed` column is untouched: textual and semantic passes are independent (see "`compressed` vs `distilled` are independent" above). |

The skip-threshold test uses a recipe-runner stub that **panics** if
called, proving the LLM path was bypassed.

---

## Out of scope

These were considered and deferred to follow-up issues:

- **Episode eviction after distillation** — PR-B marks episodes as
  distilled but does not delete them. A future retention policy can
  use the `distilled` flag plus an age threshold to safely evict.
- **A dedicated `__distill__` synthetic priority** — PR-B reuses
  `__memory__` to avoid touching the deterministic-brain routing
  contract. Splitting into its own priority is a refactor for later
  if `__memory__` becomes overloaded.
- **Confidence scores from the LLM** — emitted facts use a fixed
  `0.7` confidence. A future iteration can let the recipe return a
  per-fact confidence score.
- **Distilling old (pre-`distilled` column) episodes in bulk** — the
  lazy migration treats them as undistilled, so they flow through
  the normal pass; no special bulk-distill mode is provided.
