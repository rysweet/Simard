---
title: Episodic recall in preparation
description: How preparation_memory_operations surfaces similar past episodes via keyword-overlap search, how the results are injected into the prompt, and how self-session noise is filtered.
last_updated: 2026-07-03
owner: cognitive-memory
doc_type: reference
related:
  - ../architecture/cognitive-memory.md
  - ../architecture/episode-distillation.md
  - ./cognitive-memory-ranked-episodic-recall.md
  - ./cognitive-memory-preparation-filters.md
  - ./cognitive-memory-bootstrap-procedures.md
  - ./ooda-procedural-memory.md
  - ../memory.md
---

# Episodic recall in preparation

> **De-fork Phase 2b (#2307).** The *behavior* described here
> (keyword-overlap episodic recall via `search_episodes_by_keywords`) is
> preserved through the `CognitiveMemoryOps` trait, now backed solely by
> `LibraryCognitiveMemory`. The adapter recalls episodes from the library and
> filters with case-insensitive `content.contains`. The native
> `NativeCognitiveMemory` Cypher implementation and `src/cognitive_memory/ops.rs`
> citations on this page were deleted with the fork; treat them as historical.

> Shipped in issue [#2281](https://github.com/rysweet/Simard/issues/2281)
> as PR-C (procedural seeding + episodic recall). PR-C also reshapes
> procedural memory storage — see
> [Bootstrap procedures](./cognitive-memory-bootstrap-procedures.md).
>
> **Case-insensitivity fix:** issue
> [#2299](https://github.com/rysweet/Simard/issues/2299) repaired the
> recall path so that lowercased objective keywords match mixed-case
> stored episode content. Before the fix, recall returned **zero**
> episodes every cycle despite 20k+ stored episodes — the "0 episodes
> recalled (0 raw, 0 session-filtered)" symptom. See
> [Case-insensitive matching](#case-insensitive-matching-issue-2299).

`preparation_memory_operations` now retrieves up to **5** recent
episodes whose content overlaps with the current objective and
injects them into the brain prompt under a `Prior episodes` section.
This is the "did I encounter something similar in cycle N and do
X?" signal that the OODA brain needs to avoid re-deriving from
scratch every cycle.

> **Superseded in preparation by #2395.** OODA preparation no longer orders
> these episodes newest-first via `search_episodes_by_keywords`; it now ranks
> them with the library's multi-signal `recall_episodes_ranked` (relevance +
> confidence + importance + recency + usage + graph), reusing the per-phase
> `RecallWeightSet`. The keyword path documented here is **retained** as the
> default trait implementation and as the compressed-source UNION backfill, and
> the `session-` filter / prompt block / observability counts are unchanged. See
> [Ranked episodic recall & memory reinforcement](./cognitive-memory-ranked-episodic-recall.md).

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
    /// Blank / whitespace-only keywords are skipped, and an empty (or
    /// all-blank) keyword list returns empty without executing a query.
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

`LibraryCognitiveMemory` overrides the trait method by fetching library episodes,
lowercasing each keyword, filtering episode content with case-insensitive
`content.contains`, and capping the result at `limit`. The library returns
newest-first by temporal index, so the observable ordering remains newest-first.
Existing verbatim (mixed-case) episodes match without any write-path change or
data migration — see [Case-insensitive matching](#case-insensitive-matching-issue-2299).

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

## Case-insensitive matching (issue #2299)

Episodic recall is **case-insensitive**: a lowercased objective keyword
matches stored episode content regardless of the content's original case.
This section documents the defect that made recall return zero episodes and
the query-side fix that restores it.

### Symptom

Every OODA preparation pass logged:

```
[simard] preparation: 5 procedures, 0 episodes recalled (0 raw, 0 session-filtered)
[simard] prepared context (... 0 episodes)
```

`raw = 0` on every cycle, despite 20k+ episodes persisted in the store.
Keyword search never matched anything, so the brain re-derived context from
scratch each cycle instead of recalling "did I see something like this
before?".

### Root cause

A case-sensitivity contract violation between the write path and the search
path:

| Stage                     | Behaviour                                               |
|---------------------------|---------------------------------------------------------|
| `store_episode`           | Persists `content` **verbatim**, preserving original case (e.g. `"Merged PR #2278"`). |
| `tokenize_objective`      | **Lowercases** every keyword (step 2 of tokenization).  |
| `search_episodes_by_keywords` (before fix) | Emitted `e.content CONTAINS '<lowercased_kw>'`. |

The Ladybug (`lbug`) Cypher `CONTAINS` operator is **case-sensitive**. A
lowercased keyword such as `merged` is not a substring of the verbatim
stored content `Merged PR #2278` (capital `M`), so the predicate matched
nothing. Because the tokenizer always lowercases and the store always keeps
verbatim case, the two sides could only ever agree by accident (content that
happened to be all-lowercase). Result: `raw = 0` on essentially every cycle.

This was **not** caused by distillation draining episodes, nor by a
field-name mismatch — episodes were present and stored under `e.content`.
The single fault was case sensitivity in the comparison.

### Fix (query-side, no migration)

The fix is applied entirely on the **read/query side**, so it works against
the 20k+ episodes already persisted verbatim — no write-path change and no
data migration:

```cypher
MATCH (e:Episode)
WHERE toLower(e.content) CONTAINS '<lowercased+escaped keyword>'
   OR toLower(e.content) CONTAINS '<lowercased+escaped keyword>'
RETURN e.id, e.content, e.source_label, e.temporal_index, e.compressed
ORDER BY e.id DESC
LIMIT <limit>
```

Both sides are normalised to lower case:

1. The keyword is lowercased in Rust (`kw.to_lowercase()`), **then** passed
   through `escape_cypher` — escaping is always the last transform so the
   Cypher-injection guarantees are preserved (see [Security](#security)).
2. `toLower(e.content)` lowercases the stored content at query time.

If the embedded `lbug` engine does not support `toLower()` in a `CONTAINS`
predicate, the implementation falls back to fetching a bounded, newest-first
candidate window and filtering case-insensitively in Rust with
`content.to_lowercase().contains(&kw.to_lowercase())`. Both strategies are
query-time only and therefore both fix existing stored data identically; the
observable contract (below) is the same regardless of which is used.

> **Historical implementation note:** before #2307, the native fork used a
> Cypher `toLower(...) CONTAINS` query here. The current library adapter performs
> the same case-insensitive contract in Rust after fetching episodes from the
> library; the observable behavior remains the contract below.

### Contract

- **Case-insensitive substring**: for keyword `k` and episode content `c`,
  the episode matches iff `c.to_lowercase()` contains `k.to_lowercase()`.
- **Works on existing data**: matching applies to episodes stored verbatim
  before the fix; no re-write or migration is required.
- **Stored content is unchanged**: returned `CognitiveEpisode.content`
  preserves the original case — only the *comparison* is case-folded.
- **`raw` counts matches before the session filter**: a successful keyword
  match increments the `raw` count even if the episode is later dropped by
  the `session-` filter (see [Self-session noise filter](#self-session-noise-filter)).
- **Empty / whitespace-only keywords are dropped inside
  `search_episodes_by_keywords` itself** — defense-in-depth, not a reliance on
  the caller's tokenizer. The current code only guards the *empty slice*
  (`keywords.is_empty()`); the fix additionally trims each keyword after
  lowercasing and skips any that are blank before building a `CONTAINS` clause.
  If no keywords survive, the method returns empty without executing Cypher.
  A blank keyword can therefore never produce a match-all `CONTAINS ''` clause,
  even if a future caller bypasses `tokenize_objective`.

### Security

The injection-safety contract from PR-C is preserved unchanged. Escaping is
applied **after** lowercasing (`escape_cypher(&kw.to_lowercase())`), never
before and never omitted, so a keyword such as `' OR 1=1 //` is treated as a
literal substring and matches nothing rather than altering the query.
`limit` remains a typed integer interpolation. The query string, raw
keywords, and raw content are never logged — only counts and ids.

The blank-keyword guard (above) lives inside `search_episodes_by_keywords`,
not in `tokenize_objective`, so the public trait method is self-defending: a
caller passing `[""]` or `["   "]` can never produce a match-all
`CONTAINS ''` predicate regardless of how the keyword list was built.

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
tokens    = ["merge", "2281", "fix"]    // "and", "the" dropped (stopwords); "pr", "ci" dropped (len < 3); "2281" survives
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
tokens    = ["merge", "2281", "cog", "mem", "fix"]   // "for" dropped (stopword); "pr" dropped (len < 3)
```

Episode store contains:

| Episode          | source_label  | content                                                    |
|------------------|---------------|------------------------------------------------------------|
| epi_a            | goal-curator  | "merged PR #2278 with squashed CI fix"                     |
| epi_b            | distill:epi_b | "pr-merge pattern: enable auto-merge before review"        |
| epi_c            | session-12345 | "merge PR #2281 starting now"                              |
| epi_d            | meeting-probe | "decision: prefer merge-squash over PR rebase review"      |
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

### Example 4 — case-insensitive match (issue #2299 regression)

This is the scenario that returned **zero** episodes before the fix.

```
objective = "merge the rollback PR"
tokens    = ["merge", "rollback"]   // lowercased by the tokenizer; "the" is a stopword, "pr" is len < 3
```

Episode store contains a single verbatim, mixed-case episode:

| Episode | source_label | content                                  |
|---------|--------------|------------------------------------------|
| epi_x   | goal-curator | `"Merged the Rollback PR after CI green"` |

- **Before #2299:** `e.content CONTAINS 'merge'` is case-sensitive; the
  stored content has capital `M`/`R`, so nothing matches → `raw = 0`.
- **After #2299:** `toLower(e.content) CONTAINS 'merge'` matches
  `"merged the rollback pr after ci green"` → `raw = 1`. Because the label
  is `goal-curator` (not `session-`), it survives the filter.

Log:

```
[simard] preparation: 2 procedures, 1 episodes recalled (1 raw, 0 session-filtered)
```

The returned `CognitiveEpisode.content` is still the original
`"Merged the Rollback PR after CI green"` — only the comparison was
case-folded.

---

## Code location

| Item                                  | File                                                  |
|---------------------------------------|-------------------------------------------------------|
| `search_episodes_by_keywords` trait   | `src/cognitive_memory/mod.rs`                         |
| `LibraryCognitiveMemory` implementation| `src/cognitive_memory/library_adapter.rs`            |
| `PreparedContext.episodic_recall`     | `src/memory_consolidation/mod.rs`                     |
| Tokenizer + filter + caller           | `src/memory_consolidation/mod.rs`                     |
| Prompt injection                      | `src/ooda_actions/goal_session/advance.rs`            |
| Tests (live, library backend)         | `src/cognitive_memory/tests_pr_2299_2300_recall_triggers.rs`, |
|                                       | `src/memory_consolidation/tests_pr_c.rs`              |

---

## Testing

### Re-validation on the library backend (de-fork Phase 2b, #2307)

The native `ops.rs` and its #2299 trait-level tests were **deleted** by the
de-fork (#2307). The fix now lives query-side in
[`LibraryCognitiveMemory::search_episodes_by_keywords`](../architecture/cognitive-memory-library-adapter.md),
which lowercases both the stored episode `content` and every keyword before a
Rust-side substring match — so existing verbatim-stored episodes match
case-insensitively without a write-path migration. The live regression guards
are in `src/cognitive_memory/tests_pr_2299_2300_recall_triggers.rs`:

| Test (library backend, `LibraryCognitiveMemory::in_memory()`) | Coverage |
|---------------------------------------------------------------|----------|
| `episodic_recall_returns_nonzero_raw_for_objective_keyword`   | **Issue #2299:** store mixed-case content under a non-`session-` label, tokenize a realistic objective via `tokenize_objective`, call `search_episodes_by_keywords`, assert `raw > 0`. |
| `episodic_recall_is_case_insensitive_on_library_backend`      | Minimal case-sensitivity reproduction: ALL-CAPS content recalled by a lowercased keyword returns exactly one hit. |

### Historical trait-level tests in `src/cognitive_memory/ops.rs`

`search_episodes_by_keywords` has **no** direct trait-level coverage on the
base branch. This fix adds the three #2299 regression tests and backfills the
four substrate tests to lock down the method it modifies. The table below is a
deliverable spec, not a description of pre-existing tests.

| Test                                                | Coverage                                              | Status |
|-----------------------------------------------------|-------------------------------------------------------|--------|
| `search_episodes_by_keywords_matches_case_insensitively` | **Issue #2299 regression:** store mixed-case content with a non-`session-` label, search a lowercased keyword, assert result count > 0 (`raw > 0`). Fails on base (case-sensitive `CONTAINS`), passes after the fix. | Added by #2299 |
| `search_episodes_by_keywords_lowercase_keyword_is_injection_safe` | Lowercase path still escapes; a keyword like `' OR 1=1 //` is treated as a literal substring and matches nothing | Added by #2299 |
| `search_episodes_by_keywords_blank_keyword_emits_no_match_all` | A blank / whitespace-only keyword (e.g. `[""]`, `["   "]`) is dropped inside the method; no `CONTAINS ''` clause is built and no match-all occurs. Exercises the defense-in-depth guard. | Added by #2299 |
| `search_episodes_by_keywords_returns_substring_matches` | OR semantics across multiple keywords             | Backfilled |
| `search_episodes_by_keywords_orders_newest_first`   | Ordering by `e.id DESC`; UUID-v7 time-prefix makes this monotonically newest-first | Backfilled |
| `search_episodes_by_keywords_empty_keywords_returns_empty` | Empty slice input → empty output, no Cypher executed | Backfilled |
| `search_episodes_by_keywords_respects_limit`        | `limit=N` returns at most N rows                       | Backfilled |

### Preparation-level tests in `src/memory_consolidation/tests_pr_c.rs`

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

### Ruled out as the #2299 root cause

These candidates were investigated and confirmed **not** the cause; the
single fault was case sensitivity (above). They remain out of scope:

- **Write-path-only normalisation** (`store_episode` lowercasing content or
  writing a shadow `content_lc` field) — rejected. 20k+ episodes are already
  persisted verbatim; a write-only fix would leave all existing data
  unmatched, and re-migration is out of scope. The fix is query-side so it
  works against existing data.
- **Field-name mismatch between write and search** — ruled out. Episodes are
  written to and read from `e.content`; the field names already agree.
- **Distillation / `list_undistilled_episodes` draining or relabelling
  episodes before recall** — ruled out as the cause and explicitly untouched.
  Distillation idempotency and trigger code are owned by separate
  workstreams; this fix does not modify them.
- **20k-row data migration** — not performed; the query-side fix makes it
  unnecessary.

