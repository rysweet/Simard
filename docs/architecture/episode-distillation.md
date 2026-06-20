---
title: Episode distillation
description: How Simard periodically distills batches of recent episodes into semantic facts using an LLM-backed recipe, what concept labels are produced, when the distillation pass fires, and how distilled episodes are marked to prevent reprocessing.
last_updated: 2026-06-14
owner: simard
doc_type: concept
related:
  - ./cognitive-memory.md
  - ../reference/cognitive-memory-preparation-filters.md
  - ../reference/cognitive-memory-episodic-recall.md
  - ../reference/ooda-procedural-memory.md
  - ../reference/cognitive-memory-procedural-idempotency.md
  - ../memory.md
---

# Episode distillation

> Shipped in issue [#2281](https://github.com/rysweet/Simard/issues/2281)
> as PR-B (episode distillation). Builds on PR-A (preparation filters)
> and feeds PR-C (episodic recall) with a higher-quality semantic store.

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
Serialize as JSON, invoke recipe-runner-rs
  │
  ▼
prompt_assets/simard/recipes/distill-episodes.yaml
  │  classifies each episode into:
  │    - pr-pattern        (PR-shaped events, merge sequences, review patterns)
  │    - bug-pattern       (bug reproductions, root causes, recurring failures)
  │    - lesson-learned    (decisions, tradeoffs, things that surprised the engineer)
  │    - skip              (truly low-signal — startup logs, retries, etc.)
  ▼
Recipe output: { "facts": [ { concept, content, source_episode_id }, ... ] }
  │
  ▼
For each fact:
    store_fact(concept, content, confidence=0.7,
               concepts=[concept], source=format!("distill:{source_episode_id}"))
  │
  ▼
For EVERY input episode (even those classified "skip"):
    mark_episode_distilled(node_id)
```

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
  of the three concept labels (or `skip`) and emit a JSON object
  `{ "facts": [ { "concept": "...", "content": "...",
  "source_episode_id": "..." } ] }`.
- **Output**: parsed by the Rust caller; non-conforming output causes
  the caller to return `Err` (which then triggers the "no markers
  set" retry behaviour above).

The Rust-side invocation reuses the existing
`Command::new("recipe-runner-rs")` shape demonstrated by
`stewardship::recipe_merge_judge::RecipeMergeJudge`
(`src/stewardship/recipe_merge_judge.rs`)
and
`goal_curation::recipe_progress_checker::RecipeProgressChecker`
(`src/goal_curation/recipe_progress_checker.rs`):
the binary takes the recipe path as a positional arg followed by
zero or more `-c key=value` pairs and `AMPLIHACK_AGENT_BINARY` in
the environment. The episodes payload is passed as a single context
entry; whether it is inlined as `-c episodes=<json>` or written to a
temp file and passed as `-c episodes_path=<path>` is an
implementation detail decided at PR-B coding time by what
`recipe-runner-rs` actually supports for array values. Both shapes
satisfy the contract documented here.

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
    /// Number of episodes marked distilled after the pass.
    pub marked_count: u32,
}

impl DistillReport {
    /// The pass was skipped under threshold; no work was done.
    pub fn skipped() -> Self { Self { input_count: 0, fact_count: 0, marked_count: 0 } }

    pub fn was_skipped(&self) -> bool {
        self.input_count == 0 && self.fact_count == 0 && self.marked_count == 0
    }
}
```

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

Recipe runner exits non-zero or returns malformed JSON:

```
distill: 40 episodes pulled, recipe error: invalid JSON output, no markers set, retry next pass
```

`mark_episode_distilled` is **not** called. The same 40 episodes are
eligible on the next pass.

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
