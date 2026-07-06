---
title: Tokenized fact recall in preparation
description: How search_facts tokenizes a multi-word objective into keywords and ORs one CONTAINS clause per token so semantic facts (including goal-store:record facts) actually surface into the OODA prepared context instead of always returning zero.
last_updated: 2026-07-03
owner: cognitive-memory
related:
  - ../architecture/cognitive-memory.md
  - ./cognitive-memory-preparation-filters.md
  - ./cognitive-memory-episodic-recall.md
  - ./cognitive-memory-goal-store.md
  - ./cognitive-memory-ranked-recall.md
  - ./ooda-procedural-memory.md
  - ../memory.md
doc_type: reference
---

# Tokenized fact recall in preparation

> **De-fork Phase 2b (#2307).** The *behavior* described here (tokenized,
> multi-keyword fact recall) is preserved through the `CognitiveMemoryOps` trait,
> now backed solely by `LibraryCognitiveMemory` over the external
> `amplihack-memory` library. Native implementation details this page cites — raw
> Cypher in `src/cognitive_memory/ops.rs`, the `escape_cypher` helper, and
> `NativeCognitiveMemory` — were deleted with the fork; treat those citations as
> historical. The library-backed adapter delegates fact search to the library.

> Shipped in issue [#2302](https://github.com/rysweet/Simard/issues/2302)
> (the "facts always zero" defect). Companion to the issue #2281
> retrieval work — see
> [Preparation-phase memory filters](./cognitive-memory-preparation-filters.md)
> and [Episodic recall in preparation](./cognitive-memory-episodic-recall.md).

`search_facts` now has the tokenized-recall behavior fixed by #2302: a
multi-word objective can recall facts that share useful keywords instead of
requiring the whole objective to appear verbatim. Current source reaches this
through `LibraryCognitiveMemory`, which delegates fact search to the external
library. This is the fix for the symptom every OODA cycle logged:

```
[simard] OODA cycle: prepared context (0 facts, 0 triggers, 5 procedures, 0 episodes)
```

Facts were being **stored** correctly; they were never **recalled**.
The preparation phase passed an entire goal/objective fragment as a
single whole-string `CONTAINS` needle, and a long fragment almost never
appears verbatim as a substring of any stored fact's `concept` or
`content`. Zero rows matched, so the brain never benefited from
learned or declarative facts — including its own active goals.

After this change a realistic multi-word objective such as
`"investigate the failing CI on the auth module"` matches any fact
whose concept or content contains *any* of its keywords, and the
prepared-context fact count climbs above zero:

```
[simard] OODA cycle: prepared context (7 facts, 0 triggers, 5 procedures, 3 episodes)
```

---

## Scope

This reference covers **only** semantic fact recall — the
`CognitiveMemoryOps::search_facts` method (currently implemented by
`LibraryCognitiveMemory`) and the preparation-phase caller that feeds it.

It does **not** change:

- procedure distillation / `recall_procedure` (its own token fan-out
  already lives in `preparation_memory_operations`, see
  [OODA procedural memory](./ooda-procedural-memory.md)),
- episodic recall / `search_episodes_by_keywords` (see
  [Episodic recall in preparation](./cognitive-memory-episodic-recall.md)),
- trigger matching / `check_triggers`.

In the pre-#2307 native fork, those surfaces shared
`src/cognitive_memory/ops.rs` but were untouched by this fix. In current source,
`LibraryCognitiveMemory` delegates fact search to the library backend; the only
behavioral contract preserved here is semantic fact recall.

---

## The defect, precisely

### Before

`search_facts` built a single Cypher clause from the **whole** query
string:

```rust
// src/cognitive_memory/ops.rs (before #2302)
let q = escape_cypher(query);
self.query(&format!(
    "MATCH (f:Fact) WHERE (f.concept CONTAINS '{q}' OR f.content CONTAINS '{q}') \
     AND f.confidence >= {min_confidence} \
     RETURN f.id, f.concept, f.content, f.confidence, f.source_id, f.tags \
     ORDER BY f.id DESC LIMIT {limit}"
))?
```

The preparation phase
(`src/memory_consolidation/mod.rs`) calls this with realistic,
multi-word objective fragments:

```rust
let per_fragment = client.search_facts(fragment, 10, 0.0)?;
```

A fragment like `"investigate the failing CI on the auth module"`
is treated as one literal substring. No stored fact's `content`
contains that exact 45-character run, so the query returns nothing —
on **every** cycle, for **every** fragment.

Before the #2307 de-fork, the external library's semantic store already
tokenized the query while Simard's native `NativeCognitiveMemory` implementation
did not. The original fix closed that gap; after #2307 the library-backed adapter
is the only implementation.

### After

The observable post-fix contract is that a single shared useful keyword between
the objective and a stored fact is enough to recall that fact. In the deleted
native fork this was implemented by splitting the query and OR-ing one
`(concept CONTAINS … OR content CONTAINS …)` group per keyword; current source
delegates the search to the library backend.

---

## Method contract

`search_facts` keeps its existing signature — callers are unchanged:

```rust
fn search_facts(
    &self,
    query: &str,
    limit: u32,
    min_confidence: f64,
) -> SimardResult<Vec<CognitiveFact>>;
```

| Parameter        | Meaning                                                            |
|------------------|-------------------------------------------------------------------|
| `query`          | Free text. `"*"` is a wildcard. Tokenized-recall behavior is preserved, but the active backend owns query construction. |
| `limit`          | Maximum number of facts returned. |
| `min_confidence` | Minimum confidence floor passed through to the backend. |

The trait signature is unchanged. Historical sections below describe the deleted
native fork's `ORDER BY f.id DESC LIMIT ...` query shape; current ordering and
query construction are delegated to the library backend through
`LibraryCognitiveMemory`.

---

## Historical native tokenization contract

This section documents the deleted native `ops.rs` tokenizer that implemented
#2302. Current source delegates fact search to the external library backend, so
these rules are migration history for the observable recall contract rather than
current Simard-owned tokenizer internals.

The native `search_facts` tokenized `query` with deliberately minimal rules. The
goal was to break a natural-language objective into useful keywords **without**
disturbing the structured concept literals that internal callers pass (most
importantly `goal-store:record`).

1. **Split on ASCII whitespace only.** Internal characters such as
   `-`, `:`, `/`, `.`, `#`, and digits are preserved inside a token.
2. **Trim leading/trailing punctuation** from each token (e.g.
   `"(auth,"` → `"auth"`), preserving interior punctuation.
3. **Drop empty tokens** produced by trimming.
4. **Do not lowercase the emitted keyword.** In the native fork, the token that
   reached Cypher kept its original case so matching stayed consistent with the
   prior whole-string `CONTAINS` behaviour. The stopword test in rule 5
   is the *one* exception: it lowercases a throwaway copy of the token
   for the membership check only, so a sentence-initial `"The"` is still
   dropped while a real keyword such as `CI` keeps its case.
5. **Drop English stopwords (case-insensitively), then deduplicate.**
   Stopwords such as `the`, `on`, `and`, `for` add no signal to a
   `CONTAINS` search — every stored fact's `content` contains `the` —
   so a stopword OR-clause silently collapses the whole query into "the
   newest `LIMIT` facts" and wastes the token budget. **Short tokens are
   kept** (`CI`, `PR`, `#2302` are all discriminating in this domain),
   so there is no minimum-length filter; the keyword-vs-function-word
   distinction is what gates a token, not its length. In the deleted native
   fork, the stopword set lived in a **fact-recall-local** constant,
   `FACT_QUERY_STOPWORDS` in `src/cognitive_memory/ops.rs`, deliberately
   **separate** from the
   `TOKEN_STOPWORDS` list that the episodic/procedural `tokenize_objective`
   helper uses. Keeping it local means this fix is confined to fact search
   and provably cannot alter episodic or procedural recall (issue #2302
   scope: "only touch the fact-search function and its OODA caller"). The
   local list mirrors `TOKEN_STOPWORDS` and adds the short function words
   it omits — `on`, `of`, `to`, `in`, `a`, `an`, `at`, `is`, `are`, `by`,
   `or`, `as`, `it` — so a fragment like `"... CI on the auth module"`
   drops both `on` and `the`. Every entry is lowercase (the membership
   test compares against a lowercased copy of the token).
6. **Cap at the first 6 surviving tokens.** The cap is applied *after*
   stopword removal, so the budget is spent on discriminating keywords
   rather than on `the`/`on`. Keeps the generated Cypher bounded and
   predictable for long objectives.

> ### Why whitespace-only, and why no lowercasing
>
> This tokenizer is intentionally **not** the
> `tokenize_objective` helper used for procedural/episodic recall
> (that one splits on every non-alphanumeric run, lowercases, and
> drops short/stopword tokens). Two reasons:
>
> - **`goal-store:record` must stay a single token.** The
>   preparation phase loads goal facts with
>   `client.search_facts(GOAL_STORE_FACT_CONCEPT, GOAL_STORE_LIST_LIMIT, 0.0)`
>   where `GOAL_STORE_FACT_CONCEPT == "goal-store:record"`. That
>   string has no internal whitespace, so the whitespace-only
>   tokenizer yields exactly one token — `goal-store:record` —
>   and that load path is **byte-for-byte unchanged**. An
>   alphanumeric-splitting tokenizer would explode it into
>   `[goal, store, record]`, broadening an exact concept match into a
>   three-keyword OR that pulls in unrelated facts containing
>   "record" or "goal" and risks crowding out current goals under the
>   256-row limit. The splitting concern is separable from stopword
>   removal: dropping stopwords (rule 5) is safe for this load because
>   `goal-store:record` is not a stopword, so it survives filtering and
>   still resolves to a single token on the preserved whole-string path.
> - **Case stability.** Lowercasing would change the case-sensitivity
>   profile relative to the previous `escape_cypher(query)` whole-string
>   match. LadybugDB `CONTAINS` case behaviour is not something this
>   fix should silently alter, so the tokenizer leaves case alone.
>
> Consequence: the OODA caller in
> `src/memory_consolidation/mod.rs` needs **no code change** — only the
> single-clause `search_facts` body changes, and the stopword list it
> consults is private to that function. The goal-fact load,
> the `goal-board:snapshot` filter, the slug-dedup, and the stale-slug
> filter all keep working exactly as documented in
> [Preparation-phase memory filters](./cognitive-memory-preparation-filters.md).

### Examples

```
query  = "investigate the failing CI on the auth module"
tokens = ["investigate", "failing", "CI", "auth", "module"]   // stopwords dropped, then capped at 6
```

```
query  = "goal-store:record"
tokens = ["goal-store:record"]    // single token — exact-match path preserved
```

```
query  = "*"
tokens = (n/a)                    // wildcard branch, see below
```

```
query  = "auth"
tokens = ["auth"]                 // single token — whole-string path preserved
```

```
query  = "research:"
tokens = ["research"]             // single token — whole-string 'research:' preserved (colon NOT dropped)
```

```
query  = "the auth"
tokens = ["auth"]                 // multi-word, 1 survivor — searches keyword "auth"
```

```
query  = "the on a"
tokens = []                       // all stopwords — falls back to whole-string path
```

```
query  = "   "
tokens = []                       // whitespace only — whole-string path preserved
```

---

## Historical native query construction

The details below describe the pre-#2307 native `ops.rs` implementation that
originally fixed issue #2302. The current `LibraryCognitiveMemory` implementation
delegates to the external library; keep this section as migration history for the
observable tokenized-recall contract, not as current Simard query-building code.

The native `search_facts` chose one of four shapes based on the query and its
token count.

### 1. Wildcard (`query == "*"`)

Unchanged from before. Matches every fact above the confidence floor —
this is the export path used by `export_memory_snapshot` (issue #1710).
The wildcard is checked **before** tokenization so `*` is never treated
as a literal keyword:

```rust
MATCH (f:Fact) WHERE f.confidence >= {min_confidence}
RETURN f.id, f.concept, f.content, f.confidence, f.source_id, f.tags
ORDER BY f.id DESC LIMIT {limit}
```

### 2. Single-token or empty query (whole-string)

When the query is a **single whitespace-delimited token** (or is
empty / all stopwords, so 0 tokens survive), the **original single
whole-string clause is preserved** via `escape_cypher(query)`. This
keeps exact-concept and namespace lookups bit-for-bit identical to the
previous behaviour: empty/whitespace queries, a one-word keyword, the
`goal-store:record` exact-concept load, and trailing-colon namespace
prefixes such as `research:` and `dev-activity:`.

Preserving the whole string matters for the namespace callers
(`research_tracker::load_research_topics`,
`research_tracker::idea_extraction::extract_ideas`): they pass a literal
prefix like `"research:"` and post-filter results with
`concept.starts_with("research:")`. Dropping the trailing colon to
search the bare keyword `research` would widen the `CONTAINS` needle to
arbitrary prose mentioning "research", letting unrelated facts crowd the
`ORDER BY f.id DESC LIMIT {limit}` window and evict genuine topic facts.

```rust
let esc = escape_cypher(query);
MATCH (f:Fact)
WHERE (f.concept CONTAINS '{esc}' OR f.content CONTAINS '{esc}')
  AND f.confidence >= {min_confidence}
RETURN f.id, f.concept, f.content, f.confidence, f.source_id, f.tags
ORDER BY f.id DESC LIMIT {limit}
```

### 3. Multi-word fragment collapsing to one keyword

When the query is **multi-word** (contains whitespace) but only a single
keyword survives stopword removal — e.g. `"the auth"` -> `["auth"]` —
the surviving keyword is searched, **not** the whole `"the auth"`
literal. No stored fact contains the verbatim phrase `"the auth"`, so
the whole-string clause would return zero rows: the same "facts always
zero" symptom #2302 fixes for the multi-keyword path. This branch is
gated on the query being multi-word so the single-token namespace
lookups in case 2 are untouched.

```rust
// for query "the auth" -> tokens ["auth"]
let esc = escape_cypher("auth");
MATCH (f:Fact)
WHERE (f.concept CONTAINS '{esc}' OR f.content CONTAINS '{esc}')
  AND f.confidence >= {min_confidence}
RETURN f.id, f.concept, f.content, f.confidence, f.source_id, f.tags
ORDER BY f.id DESC LIMIT {limit}
```

### 4. Two or more tokens

The historical native fix path used one `(concept CONTAINS … OR content CONTAINS …)` group
per token, OR-joined, each token escaped with the same native `escape_cypher`
helper that `search_episodes_by_keywords` used in the deleted `ops.rs` file:

```rust
// for tokens ["investigate", "failing", "auth"]
MATCH (f:Fact)
WHERE (
       (f.concept CONTAINS 'investigate' OR f.content CONTAINS 'investigate')
    OR (f.concept CONTAINS 'failing'     OR f.content CONTAINS 'failing')
    OR (f.concept CONTAINS 'auth'        OR f.content CONTAINS 'auth')
  )
  AND f.confidence >= {min_confidence}
RETURN f.id, f.concept, f.content, f.confidence, f.source_id, f.tags
ORDER BY f.id DESC LIMIT {limit}
```

The `AND f.confidence >= {min_confidence}` floor and the
`ORDER BY f.id DESC LIMIT {limit}` tail are appended exactly once, after
the OR group — they are not per-token.

### Escaping

In the deleted native fork, every token (and the whole-string query in case 2)
was passed through `escape_cypher` before interpolation. That escaped `\`, `'`,
newline, carriage-return, tab, and null, preventing query breakage and Cypher
injection in the native query builder. Current escaping/query construction is
owned by the external library backend.

---

## How this surfaces facts into `PreparedContext`

`PreparedContext` is unchanged — no new field, no shape change:

```rust
pub struct PreparedContext {
    pub relevant_facts: Vec<CognitiveFact>,
    // ... triggers, procedures, episodic_recall ...
}
```

The preparation phase
(`preparation_memory_operations_with_active_slugs` in
`src/memory_consolidation/mod.rs`) builds `relevant_facts` from two
`search_facts` calls, both of which now benefit from tokenization:

1. **Per-objective-fragment recall.** The objective is split on `"; "`
   into fragments; each fragment is passed to
   `client.search_facts(fragment, 10, 0.0)`. With tokenization, a
   multi-word fragment matches any fact sharing a keyword — this is the
   call that previously always returned zero.
2. **Goal-fact load.** `client.search_facts(GOAL_STORE_FACT_CONCEPT, GOAL_STORE_LIST_LIMIT, 0.0)`
   loads active goal records. Because `goal-store:record` has no
   whitespace it tokenizes to a single token and stays on the
   preserved exact-match path — its results are identical to before.

The existing post-search filters then run unchanged: drop
`goal-board:snapshot` revisions, dedup `goal-store:record` by slug
keeping the latest, drop stale slugs not on the live goal-board, and
truncate to 10. See
[Preparation-phase memory filters](./cognitive-memory-preparation-filters.md)
for that pipeline.

Net effect: active-goal facts and keyword-relevant learned facts both
land in `relevant_facts`, and the per-cycle log shows a non-zero fact
count.

---

## Worked example

### Store

```rust
let mem = LibraryCognitiveMemory::in_memory()?;

// A learned fact whose CONTENT shares a keyword with the objective,
// but whose content does NOT contain the whole objective verbatim.
mem.store_fact(
    "ci-pattern",
    "the auth module integration tests are flaky under heavy load",
    0.8,
    Some("episode-1"),
)?;

// A goal record, filed under the goal-store:record concept.
mem.store_fact(
    "goal-store:record",
    &serde_json::to_string(&GoalRecord {
        slug: "fix-auth".into(),
        title: "Stabilize auth module tests".into(),
        rationale: "flaky CI blocks merges".into(),
        status: GoalStatus::Active,
        priority: 1,
        owner_identity: "simard".into(),
        source_session_id: session_id.clone(),
        updated_in: SessionPhase::Reflection,
    })?,
    1.0,
    None,
)?;
```

### Recall

```rust
let objective = "investigate the failing auth module CI";
let active: HashSet<&str> = ["fix-auth"].into_iter().collect();

let prepared = preparation_memory_operations_with_active_slugs(
    objective, &session_id, &mem, Some(&active),
)?;

assert!(prepared.relevant_facts.len() > 0);                       // was 0 before #2302
assert!(prepared.relevant_facts.iter().any(|f| f.concept == "ci-pattern"));
assert!(prepared.relevant_facts.iter().any(|f| f.concept == "goal-store:record"));
```

**Before #2302:** `relevant_facts` is empty — the 38-char objective
matches no fact substring, and the goal load (an exact-match path) was
the only thing that ever returned rows.

**After #2302:** the objective tokenizes to
`["investigate", "failing", "auth", "module", "CI"]` (the stopword
`the` is dropped); the `auth` and `module` tokens match the
`ci-pattern` fact's content (its lowercase content has no `CI`
substring, and matching is case-sensitive), and the `fix-auth` goal
record loads as before. Both surface.

---

## Observability

The existing per-cycle OODA summary now reports a meaningful fact
count instead of a permanent zero. The line is emitted by
`src/ooda_loop/cycle.rs` (not the preparation module):

```
[simard] OODA cycle: prepared context (7 facts, 0 triggers, 5 procedures, 3 episodes)
```

To confirm the fix in daemon logs, `grep "OODA cycle: prepared context"`
and verify the leading fact count is non-zero when facts have been
stored. A persistent `(0 facts, …)` after facts exist indicates a
regression in this path. Do not confuse this with the separate
`[simard] preparation: N procedures, N episodes recalled (…)` line
from `src/memory_consolidation/mod.rs`, which carries no fact count.

`search_facts` also emits debug spans on entry and exit:

```
search_facts: starting query   query_len=38 is_wildcard=false
search_facts: query complete   result_count=7
```

A `result_count=0` on a multi-word, non-wildcard query when facts are
known to exist is the signature of the original defect.

---

## Historical native preserved invariants (regression guards)

For the pre-#2307 native fix, this change was intentionally narrow. The
following behaviours were unchanged and protected by tests; current source
preserves the high-level recall contract through the library backend:

- **`*` wildcard export** still returns all facts above the floor
  (issue #1710 — `export_memory_snapshot`).
- **`min_confidence` floor** and the native `ORDER BY f.id DESC LIMIT {limit}`
  tail stayed identical in the deleted native query builder.
- **Single-keyword and empty/whitespace queries** took the preserved
  whole-string path in the native query builder — results unchanged there.
- **`goal-store:record` load** (limit 256, dedup-by-slug,
  `goal-board:snapshot` filter, stale-slug filter) is byte-for-byte
  unchanged because the concept tokenizes to one token.
- **Query safety** for fact content and objective text remains owned by the
  active backend. In the deleted native fork this was the shared
  `escape_cypher` helper; current code delegates query construction to the
  library backend.

---

## Code location

| Item                                      | File                                          |
|-------------------------------------------|-----------------------------------------------|
| `search_facts` implementation             | `src/cognitive_memory/library_adapter.rs` (`LibraryCognitiveMemory`, delegating to the library) |
| Historical native query builder / `escape_cypher` | `src/cognitive_memory/ops.rs` (deleted in #2307; history only) |
| Preparation caller(s)                     | `src/memory_consolidation/mod.rs`             |
| `PreparedContext` struct                  | `src/memory_consolidation/mod.rs`             |
| `GOAL_STORE_FACT_CONCEPT` / list limit    | `src/goals/cognitive_memory_store.rs`         |
| `GoalRecord` shape                        | `src/goals/types.rs`                          |
| Current backend tests                     | `src/cognitive_memory/tests_library_parity.rs` and related library-adapter tests |
| Preparation-level tests                   | `src/memory_consolidation/tests_pr_a.rs` (new test), `tests.rs` (existing guards) |

---

## Testing

### Historical native trait-level tests in `src/cognitive_memory/ops.rs`

| Test                                                  | Coverage                                                                 |
|-------------------------------------------------------|--------------------------------------------------------------------------|
| `search_facts_recalls_on_shared_keyword`              | **New, failing-first.** A multi-word query whose full text is NOT a substring of any fact still recalls a fact sharing one keyword. Fails before the fix (0 rows), passes after. |
| `search_facts_by_content`                             | Existing single-keyword content match — must keep passing.               |
| `search_facts_respects_limit`                         | `limit=N` returns at most N rows — unchanged.                            |
| `search_facts_empty_result_for_no_match`              | A query sharing no keyword with any fact returns empty.                  |
| `search_facts_wildcard_returns_all`                   | `"*"` wildcard export path — unchanged.                                  |

### Preparation-level tests in `src/memory_consolidation/tests_pr_a.rs`

This file already holds the active-slug preparation tests
(`preparation_drops_stale_goal_store_records`,
`preparation_keeps_active_goal_store_records`) and the shared
`prep_with_active_slugs` harness, so the new active-slug test belongs
here rather than in `tests.rs`.

| Test                                                  | Coverage                                                                 |
|-------------------------------------------------------|--------------------------------------------------------------------------|
| `preparation_recalls_keyword_and_goal_facts`          | **New, failing-first.** Reuses the `prep_with_active_slugs` harness: stores a keyword-bearing fact and a valid `goal-store:record` fact, then prepares with a realistic multi-word objective and `Some({"fix-auth"})`; asserts `relevant_facts.len() > 0` and that both facts surface. Fails before the fix. |
| Existing goal-fact dedup / stale-slug / snapshot tests (spanning `tests.rs` and `tests_pr_a.rs`) | Must keep passing — the goal-store load path is unchanged (including the limit ≥ 256 assertion in `preparation_uses_goal_store_list_limit_not_hardcoded_20`). |

Run the two affected modules:

```bash
cargo test cognitive_memory
cargo test memory_consolidation
```

Both must be green after the fix, and the two new tests must fail
when run against the pre-fix `search_facts` body (TDD: red → green).

---

## Out of scope

Deferred deliberately so this fix stays focused on fact recall:

- **Stemming and embedding normalization.** The tokenizer drops
  stopwords and dedups but does not stem (`failing` ≠ `fails`) or
  fold synonyms. A future PR could share more of `tokenize_objective`'s
  normalization *if* fact-search precision needs it — but only after
  re-checking the `goal-store:record` single-token constraint (the
  whitespace split, not the stopword drop, is what preserves it).
- **Embedding / semantic ranking.** Keyword OR is sufficient to fix the
  "always zero" defect. Embedding-based similarity is the same
  follow-up noted for episodic recall.
- **Per-keyword relevance scoring.** Results are ordered newest-first
  (`f.id DESC`); there is no per-fact relevance score. A scoring layer
  can be added without changing this method's signature.
- **Procedure, episode, and trigger recall.** Untouched here — see
  [OODA procedural memory](./ooda-procedural-memory.md) and
  [Episodic recall in preparation](./cognitive-memory-episodic-recall.md).
