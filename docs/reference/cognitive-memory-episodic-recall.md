---
title: Episodic recall in preparation
description: How preparation_memory_operations surfaces similar past episodes via keyword-overlap search, how the results are injected into the prompt, and how self-session noise is filtered.
last_updated: 2026-06-14
owner: simard
doc_type: reference
related:
  - ../architecture/cognitive-memory.md
  - ../architecture/episode-distillation.md
  - ./cognitive-memory-preparation-filters.md
  - ./cognitive-memory-bootstrap-procedures.md
  - ./ooda-procedural-memory.md
  - ../memory.md
---

# Episodic recall in preparation

> Shipped in issue [#2281](https://github.com/rysweet/Simard/issues/2281)
> as PR-C (procedural seeding + episodic recall). PR-C also reshapes
> procedural memory storage — see
> [Bootstrap procedures](./cognitive-memory-bootstrap-procedures.md).

`preparation_memory_operations` now retrieves up to **5** recent
episodes whose content overlaps with the current objective and
injects them into the brain prompt under a `Prior episodes` section.
This is the "did I encounter something similar in cycle N and do
X?" signal that the OODA brain needs to avoid re-deriving from
scratch every cycle.

Episodic recall complements PR-B's distillation: distilled facts
capture *patterns* across many episodes, while episodic recall
surfaces *specific* past situations. Both arrive in the
`PreparedContext`; the brain decides which to consult.

---

## Trait method

PR-C adds one method to `CognitiveMemoryOps`
(`src/cognitive_memory/mod.rs`) with a default no-op so legacy
bridges keep compiling:

```rust
pub trait CognitiveMemoryOps {
    // ... existing methods ...

    /// Return up to `limit` recent episodes whose `content` contains
    /// at least one of the supplied keywords (case-insensitive
    /// substring). Newest first.
    ///
    /// Default impl returns empty so legacy backends keep compiling.
    fn search_episodes_by_keywords(
        &self,
        keywords: &[String],
        limit: u32,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        Ok(vec![])
    }
}
```

`NativeCognitiveMemory` overrides with a Cypher query that ORs one
`e.content CONTAINS '<escaped>'` clause per keyword, orders by
descending `e.id` (newest first — UUID-v7 ids are time-prefixed, so
descending lex-sort is the same as newest-by-creation), and caps the
result at `limit`. No reliance on `temporal_index` is required for
the ordering.

### Why keyword overlap, not embeddings

PR-C deliberately uses keyword overlap rather than embedding-based
similarity:

- Cypher `CONTAINS` is already available; no new index, no embedding
  pipeline, no model dependency.
- The objective text is short and concrete (PR numbers, file paths,
  action verbs); keyword overlap captures the bulk of useful signal.
- Distilled summaries (PR-B) inflate substring-hit rate, so the
  signal-to-noise ratio is already improving over time without
  touching the search algorithm.

Embedding-based similarity is an obvious follow-up if keyword
overlap proves insufficient in practice. The trait method shape
(`keywords: &[String]`) is the smallest contract that lets a future
implementation swap in semantic ranking without forcing every caller
to be embedding-aware. Callers that want semantic ranking would call
a different method; the existing one will remain.

---

## `PreparedContext` extension

`PreparedContext` (in `src/memory_consolidation/mod.rs`) gains one
field:

```rust
pub struct PreparedContext {
    pub relevant_facts: Vec<Fact>,
    pub triggered_prospectives: Vec<Trigger>,
    pub recalled_procedures: Vec<Procedure>,
    pub episodic_recall: Vec<CognitiveEpisode>, // NEW in PR-C
}
```

All other `PreparedContext` constructors in the codebase (test stubs,
snapshot importers, mock bridges) are updated to add
`episodic_recall: vec![]` so compilation stays green.

---

## Search behaviour

### Tokenization

`preparation_memory_operations` tokenizes the current objective into
keywords before calling `search_episodes_by_keywords`:

1. Split on non-alphanumeric runs.
2. Lowercase each token.
3. Drop tokens shorter than 3 characters.
4. Drop common English stopwords: `the, and, for, with, this, that,
   from, has, was, were, will, into, when, where, what, why, how`.
5. Deduplicate.

Example:

```
objective = "merge PR #2281 and fix the CI"
tokens    = ["merge", "pr", "fix"]    // "and", "the" dropped; "ci" too short; "2281" survives
```

(The "#" prefix is stripped during tokenization, so `#2281` becomes
`2281` — useful when prior episodes reference the same PR number.)

### Query and limit

The tokenized list is passed to
`search_episodes_by_keywords(&tokens, 5)`. The top 5 newest matches
are retained. There is no minimum-overlap floor beyond "at least one
keyword matches"; the OR semantics intentionally err toward recall
over precision because the brain is the final filter.

### Self-session noise filter

Episodes whose `source_label` begins with `session-` are filtered
out of the recall result inside `preparation_memory_operations`,
*after* the trait call returns:

```rust
let recalled = bridge
    .search_episodes_by_keywords(&tokens, 5)?
    .into_iter()
    .filter(|e| !e.source_label.starts_with("session-"))
    .collect();
```

Rationale: `session-` episodes are written by the very session loop
that is now preparing; surfacing them back into the prompt creates a
self-reinforcing loop where the brain sees its own recent breath as
"prior knowledge". Episodes from `goal-curator`, `consolidation`,
distillation source labels (`distill:…`), and meeting probes pass
through.

---

## Prompt injection

`src/ooda_actions/goal_session/advance.rs` is the single site that
renders `PreparedContext` into the brain prompt. After the existing
facts/prospectives/procedures sections, PR-C adds a fourth block —
**omitted entirely when `episodic_recall` is empty** to avoid empty
section noise:

```
## Prior episodes (most-recent first)
- [goal-curator] [t=1749745320] merged PR #2278 by squashing CI fix + scope cleanup in one revision
- [distill:epi_4f2a] [t=1749659473] pr-merge pattern: enable auto-merge before final review reduces CI re-runs
- [consolidation] [t=1749590867] tests: cargo nextest run --workspace --no-fail-fast catches flakes earlier
```

Each line follows the format `- [{source_label}] [t={temporal_index}] {content_truncated}` where:

- `{temporal_index}` is the monotonic `i64` index that lives on every
  `CognitiveEpisode`. It is **not** wall-clock time — the
  `CognitiveEpisode` struct deliberately carries no `recorded_at`
  field; only relative order is preserved. Operators who need
  wall-clock time can correlate via daemon logs.
- `{content_truncated}` is the episode content truncated to 200
  characters with an ellipsis when longer.

When `episodic_recall` is empty, the `## Prior episodes` heading is
not emitted at all — keeping the prompt clean.

---

## Observability

A single log line summarises the recall outcome per preparation pass:

```
[simard] preparation: 4 procedures, 3 episodes recalled (5 raw, 2 session-filtered)
```

Fields:

| Field                  | Meaning                                                       |
|------------------------|---------------------------------------------------------------|
| `procedures`           | Final length of `PreparedContext.recalled_procedures`         |
| `episodes recalled`    | Final length of `PreparedContext.episodic_recall`             |
| `raw`                  | Count returned by `search_episodes_by_keywords` before filter |
| `session-filtered`     | Count dropped by the `source_label.starts_with("session-")` filter |

When the objective produces no keywords (very short objective,
all stopwords), the line still emits with zero counts; no trait call
is made.

---

## Examples

### Example 1 — PR-merge objective

```
objective = "merge PR #2281 for cog-mem fix"
tokens    = ["merge", "pr", "2281", "cog", "mem", "fix"]
```

Episode store contains:

| Episode          | source_label  | content                                                    |
|------------------|---------------|------------------------------------------------------------|
| epi_a            | goal-curator  | "merged PR #2278 with squashed CI fix"                     |
| epi_b            | distill:epi_b | "pr-merge pattern: enable auto-merge before review"        |
| epi_c            | session-12345 | "merge PR #2281 starting now"                              |
| epi_d            | meeting-probe | "decision: prefer PR review squash over rebase"            |
| epi_e            | goal-curator  | "ran cargo bench unrelated"                                |

Raw search returns 4 hits (a, b, c, d match a keyword). After
`session-` filter: 3 (epi_c dropped). `PreparedContext.episodic_recall`
contains `[epi_a, epi_b, epi_d]` (newest first).

Log:

```
[simard] preparation: 4 procedures, 3 episodes recalled (4 raw, 1 session-filtered)
```

### Example 2 — no keyword matches

```
objective = "review notes"
tokens    = ["review", "notes"]
```

No episodes contain either substring → recall is empty, no `## Prior
episodes` block is emitted to the brain prompt.

Log:

```
[simard] preparation: 2 procedures, 0 episodes recalled (0 raw, 0 session-filtered)
```

### Example 3 — extremely short objective

```
objective = "go"
tokens    = []           // single token < 3 chars dropped
```

No trait call is made; recall is empty.

Log:

```
[simard] preparation: 0 procedures, 0 episodes recalled (0 raw, 0 session-filtered)
```

---

## Code location

| Item                                  | File                                                  |
|---------------------------------------|-------------------------------------------------------|
| `search_episodes_by_keywords` trait   | `src/cognitive_memory/mod.rs`                         |
| `NativeCognitiveMemory` implementation| `src/cognitive_memory/ops.rs`                         |
| `PreparedContext.episodic_recall`     | `src/memory_consolidation/mod.rs`                     |
| Tokenizer + filter + caller           | `src/memory_consolidation/mod.rs`                     |
| Prompt injection                      | `src/ooda_actions/goal_session/advance.rs`            |
| Tests                                 | `src/cognitive_memory/ops.rs`,                         |
|                                       | `src/memory_consolidation/tests.rs`                    |

---

## Testing

### Trait-level tests in `src/cognitive_memory/ops.rs`

| Test                                                | Coverage                                              |
|-----------------------------------------------------|-------------------------------------------------------|
| `search_episodes_by_keywords_returns_substring_matches` | OR semantics across multiple keywords             |
| `search_episodes_by_keywords_orders_newest_first`   | Ordering by `e.id DESC`; UUID-v7 time-prefix makes this monotonically newest-first |
| `search_episodes_by_keywords_empty_keywords_returns_empty` | Empty input → empty output, no Cypher executed |
| `search_episodes_by_keywords_respects_limit`        | `limit=N` returns at most N rows                       |

### Preparation-level tests in `src/memory_consolidation/tests.rs`

| Test                                                | Coverage                                              |
|-----------------------------------------------------|-------------------------------------------------------|
| `preparation_injects_episodic_recall`               | Objective with "merge" + matching episode → recall populated |
| `preparation_excludes_self_session_noise`           | `source_label = "session-..."` episodes filtered      |
| `preparation_tokenizes_and_strips_stopwords`        | Tokenizer rules verified end-to-end                   |
| `preparation_emits_no_recall_when_objective_yields_no_tokens` | Edge case: short / stopword-only objective    |

---

## Out of scope

- **Embedding-based similarity** — keyword overlap is sufficient
  per the PR-C brief. Trait shape allows a future drop-in.
- **Per-episode relevance scoring** — current implementation does
  not score; result order is newest-first. A scoring layer (Jaccard,
  BM25) can be added without changing the trait.
- **Cross-session deduplication** — when two near-identical episodes
  appear (e.g. textual dedup didn't catch them), both will surface.
  PR-B's distillation reduces this over time.
- **Cross-repo recall** — recall is scoped to the local cognitive
  memory file; multi-agent hive-mind sharing is a separate concern
  (see `docs/architecture/cognitive-memory.md` on hive mind).
