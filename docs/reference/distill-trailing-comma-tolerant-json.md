---
title: Trailing-comma-tolerant recipe-envelope JSON parsing
description: Reference for the string-literal-aware strip_json_trailing_commas primitive in recipe_output::extract and the fallback-only parse_recipe_envelope retry it powers in the distillation pass — the mechanism that recovers a distill { "facts": [...] } object emitted with a structural trailing comma, eliminating the recurring 100% distill parse-fail anomaly (#2658) that blocked the kgpacks-rs parity goals and skipped the quality gym gate. Covers the primitive's clean-path Cow::Borrowed no-op guarantee, its removal-only / fail-closed semantics, the strict-first-then-retry wiring at both scan_cleaned_for_facts parse sites, telemetry, the security model for untrusted LLM output, and the regression tests.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./distill-recipe-output-capture.md
  - ./distill-raw-capture-on-parse-failure.md
  - ./text-parsing-wire-formats.md
  - ../architecture/episode-distillation.md
  - ../concepts/copilot-launcher-preamble-stripping.md
  - ../concepts/overseer-root-cause-why.md
  - ../../src/recipe_output/extract.rs
  - ../../src/recipe_output/mod.rs
  - ../../src/memory_consolidation/distillation.rs
---

# Trailing-comma-tolerant recipe-envelope JSON parsing

> **Status: implemented — resolves [#2658](https://github.com/rysweet/Simard/issues/2658)**
> (the recurring `overseer-obs:anomaly:distill parse-fail rate 100%` signature
> the Overseer saw twice in cognitive memory,
> [#2678](https://github.com/rysweet/Simard/issues/2678)). The doc is coupled to
> its code and lands in the **same PR/commit** as the change it describes, so the
> present-tense description below is the shipped behavior. Locations:
> primitive + unit tests
> [`src/recipe_output/extract.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/extract.rs);
> re-export
> [`src/recipe_output/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/mod.rs);
> consumer + integration tests
> [`src/memory_consolidation/distillation.rs`](https://github.com/rysweet/Simard/blob/main/src/memory_consolidation/distillation.rs).

The distillation pass turns batches of episodic memory into semantic facts by
asking the distill agent for a `{ "facts": [...], "procedures": [...] }` object
(the [`RecipeEnvelope`](./distill-recipe-output-capture.md)) and parsing it with
strict `serde_json`. Strict `serde_json` rejects a **trailing comma** — a `,`
immediately before a closing `}` or `]`. When the agent emits
`{"facts":[ … ],}` (a near-universal LLM JSON quirk), `serde_json::from_str`
returns `Err` for the *entire* object, `parse_facts_document` returns `Err`, the
batch is deferred, and the pass records a parse failure. Every cycle repeats the
same miss, so `distill_parse_success_rate` pins to `0`, the Overseer emits
`anomaly:distill parse-fail rate 100%`, the blocked kgpacks-rs parity goals
(#12 / #16 / #17 / #18) never clear the process gate, and the quality **gym
gate is skipped** as a downstream side-effect.

This feature closes that gap with a single, narrow, string-literal-aware
primitive — `strip_json_trailing_commas` — and a **fallback-only** retry. Strict
parsing still runs first and is never weakened; only *after* a strict miss does
the parser retry against a comma-stripped view of the same bytes. A well-formed
object never touches the fallback and is byte-for-byte unchanged; a genuinely
malformed object still fails closed and defers the batch, exactly as before.

For the envelope contract this relaxes, see
[Distill recipe output capture](./distill-recipe-output-capture.md). For the
shared noise-stripping chokepoint that runs *before* this parse, see
[Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md)
and [Text-parsing wire formats](./text-parsing-wire-formats.md).

## Contents

- [Why](#why)
- [Design in one line](#design-in-one-line)
- [Primitive: `strip_json_trailing_commas`](#primitive-strip_json_trailing_commas)
  - [Signature and guarantees](#signature-and-guarantees)
  - [What counts as a structural trailing comma](#what-counts-as-a-structural-trailing-comma)
  - [Clean-path no-op](#clean-path-no-op)
- [Consumer: `parse_recipe_envelope`](#consumer-parse_recipe_envelope)
- [Where it is wired](#where-it-is-wired)
- [Configuration](#configuration)
- [Telemetry](#telemetry)
- [Security model](#security-model)
- [Examples](#examples)
- [When the fallback does *not* fire](#when-the-fallback-does-not-fire)
- [Tests](#tests)
- [Related](#related)

## Why

The #2484 / #2496 / #2622 work already routes distill output through the shared
hardened extractor
([`src/recipe_output/extract.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/extract.rs))
— it strips ANSI colour codes, timestamped log lines, the runner banner, and
the Copilot launch preamble, then returns balanced `{…}` spans. That eliminated
the *noise* class of parse failure. What remained was a **content** class:
the recovered span is the agent's real answer, is well-formed apart from one
structural trailing comma, and is still rejected wholesale by strict
`serde_json`.

A trailing comma is the single most common well-formed-intent JSON defect LLMs
produce. Rejecting the whole object over it discards a complete, attributed
answer and defers the batch every cycle — the exact `100%` parse-fail signature
the Overseer flagged. Recovering it, and *only* it, restores the pass without
loosening the parser for anything else.

## Design in one line

Keep strict `serde_json` as the source of truth; on a strict miss, retry the
same bytes once with structural trailing commas removed; on a second miss, fail
closed and defer the batch.

## Primitive: `strip_json_trailing_commas`

### Signature and guarantees

```rust
// src/recipe_output/extract.rs — peer of strip_ansi / strip_recipe_noise / balanced_objects

/// Remove **structural** trailing commas — a `,` whose next non-whitespace byte
/// is `}` or `]`, outside any JSON string literal — from `s`.
///
/// String-literal aware: a `,` inside `"…"` (honouring `\` escapes) is never
/// touched, so comma-containing fact text is preserved exactly. Removal-only:
/// the output bytes are a subset of the input bytes — no byte is ever inserted,
/// reordered, or rewritten, so the function can never inject content the model
/// did not emit.
///
/// Single-pass, O(n), infallible (total over every byte sequence, never
/// panics), UTF-8-preserving (only the ASCII byte `,` / `0x2C` is dropped;
/// conversion back is a checked `String::from_utf8`, no `unsafe`).
///
/// Returns [`Cow::Borrowed`] unchanged when the input has no removable
/// structural comma — the zero-allocation clean path.
pub fn strip_json_trailing_commas(s: &str) -> Cow<'_, str>;
```

Re-exported from the module so consumers call it as
`crate::recipe_output::strip_json_trailing_commas`:

```rust
// src/recipe_output/mod.rs
pub use extract::{
    VerdictMatch, balanced_objects, extract_json_payload, extract_verdict,
    last_balanced_object, strip_ansi, strip_json_trailing_commas, strip_recipe_noise,
};
```

| Property | Guarantee |
| --- | --- |
| Infallible | Total over all `&str` inputs; never panics (verified by empty / huge / lone-`\` / all-comma / non-ASCII negative tests). |
| Bounded | Strictly O(n) single pass. No repair-until-stable loop, no backtracking — no algorithmic-complexity amplification. |
| Removal-only | Output bytes ⊆ input bytes. Cannot add, reorder, or rewrite bytes. |
| String-safe | Uses the same `{ in_string, escaped }` state machine as the proven [`scan_balanced`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/extract.rs); a `,` inside a string literal is preserved. |
| UTF-8-safe | Only the ASCII `,` (`0x2C`) is dropped; result is rebuilt with a checked `String::from_utf8`. No `unsafe`, no `from_utf8_unchecked`, no boundary-splitting index. |
| Clean-path no-op | `Cow::Borrowed` on input with no removable structural comma — zero allocation, byte-identical. |

### What counts as a structural trailing comma

A comma is removed **iff** all of the following hold:

1. It is the ASCII byte `,` (`0x2C`), **and**
2. the scanner is **not** inside a string literal (tracking `"` open/close and
   `\` escapes), **and**
3. its next **non-whitespace** byte is `}` or `]`.

Everything else is left exactly in place. In particular:

- A comma **inside** a string (`"a,b"`) is content, not structure → **kept**.
- A comma between two values (`[1, 2]`, `{"a":1, "b":2}`) is not trailing →
  **kept**.
- **Multiple structural trailing commas are all removed in the one pass**, as
  long as they are not adjacent. The single left-to-right scan tests every comma
  independently, so each structural trailing comma is dropped when the cursor
  reaches it — there is no per-document limit of one. The dominant real defect is
  exactly this shape: a comma after the last array element *and* a comma after
  the last object member, e.g.

  ```json
  { "facts": [ { "concept": "x", "source_episode_id": "ep-1" }, ], }
  ```

  Both commas are structural (each one's next non-whitespace byte is `]` or `}`
  respectively) and **non-adjacent**, so the single pass removes both, yielding
  strict-valid JSON `{ "facts": [ { … } ] }`. Likewise `{ "a": [1, 2, ], }`
  (a trailing comma inside the array *and* one after it) is fully repaired in one
  pass to `{ "a": [1, 2 ] }`.
- The single unrepaired case is an **adjacent** run of commas — a `,`
  immediately before another `,`. In a single left-to-right pass over `[1,,]`
  only the last comma qualifies (its next non-whitespace byte is `]`); it is
  removed, leaving `[1,]`. The now-trailing first comma is **not** revisited —
  the primitive does not iterate to a fixpoint — so the span still fails strict
  parsing. The limitation is therefore **adjacency** (comma-immediately-before-comma),
  not the *number* of trailing commas. This is deliberate: the fix targets the
  observed well-formed-intent defect, not arbitrary JSON repair (see
  [When the fallback does *not* fire](#when-the-fallback-does-not-fire)).

### Clean-path no-op

`strip_json_trailing_commas` scans for a removable structural comma **before**
allocating. If none exists it returns `Cow::Borrowed(s)` — the exact input, zero
allocation. Because the fallback retry only runs *after* strict parsing has
already missed (see below), a well-formed object never reaches the stripper at
all, and even when it does — during the retry of a genuinely malformed object —
a clean substring is passed through byte-for-byte. Adopting the feature changes
**no** behavior on well-formed distill output.

## Consumer: `parse_recipe_envelope`

A private helper in `distillation.rs` centralises the strict-first-then-retry
policy so both parse sites share one implementation (DRY) and one contract:

```rust
// src/memory_consolidation/distillation.rs

/// Parse one candidate span as a `RecipeEnvelope`, strict first.
///
/// Strict `serde_json::from_str::<RecipeEnvelope>` runs first and is never
/// weakened. **Only** on `Err` is the span retried against its
/// trailing-comma-stripped view. Returns `None` when both the strict parse and
/// the comma-stripped retry fail, preserving the fail-closed deferral contract
/// — a genuinely malformed span yields `None`, the batch is deferred, and
/// nothing hollow is persisted.
fn parse_recipe_envelope(candidate: &str) -> Option<RecipeEnvelope>;
```

Semantics:

- **Fallback-only.** The comma-stripped retry runs *only* after a strict miss.
  A span that parses strictly never touches the stripper.
- **Single retry.** Exactly one comma-stripped attempt — no repair loop.
- **Fail-closed.** `None` on unrecoverable input. No hollow `Ok`, no
  empty-but-successful envelope invented from garbage. The existing
  `Err`/`None` → batch-deferred contract of
  [`parse_facts_document`](https://github.com/rysweet/Simard/blob/main/src/memory_consolidation/distillation.rs)
  is preserved unchanged.

## Where it is wired

`parse_recipe_envelope` replaces the two bare
`serde_json::from_str::<RecipeEnvelope>` calls in
[`scan_cleaned_for_facts`](https://github.com/rysweet/Simard/blob/main/src/memory_consolidation/distillation.rs),
leaving the surrounding candidate-selection logic untouched:

1. **Fast path** — the cleaned text *is* the JSON object:

   ```rust
   if let Some(env) = parse_recipe_envelope(trimmed) {
       return Some(env.into_output());
   }
   ```

2. **Slow path** — each balanced `{…}` span from
   [`balanced_objects`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/extract.rs),
   scanned from the end, is parsed the same way. `parse_recipe_envelope` returns
   `Option<RecipeEnvelope>`, so the tier logic inspects the envelope's
   `into_output()` exactly as the pre-existing strict path did:

   ```rust
   for span in crate::recipe_output::balanced_objects(trimmed).into_iter().rev() {
       if let Some(env) = parse_recipe_envelope(span) {
           let output = env.into_output();
           // unchanged tier selection over output.facts / output.procedures …
       }
   }
   ```

   The three preference tiers are preserved exactly:

   ```
   1. last object with a grounded-capable fact (non-empty source_episode_id)
   2. last otherwise-non-empty object (facts / procedures present)
   3. last parseable empty { "facts": [] } object
   ```

Span boundaries are unaffected: `balanced_objects` counts braces
string-literal-aware and never splits on a comma, so a trailing comma changes
only the *inner* strict parse, which is now the relaxed one. The grounded-capable
→ non-empty → empty tier ordering, and the reason it exists (a source-less
object must not shadow a fully-attributed answer), are unchanged — see
[Distill recipe output capture](./distill-recipe-output-capture.md).

The "single pass" guarantee is a property of the **primitive** — one O(n) scan
of the bytes it is handed. It is *not* a claim of one retry per document: on the
slow path the strict-then-strip retry is attempted per balanced span that misses
strict parse, where previously a missing span was simply skipped. Because
`balanced_objects` yields **disjoint** spans, the total stripper work stays
linear in the cleaned text; the only cost is that an adversarial input made of
many failing spans doubles the number of *parse attempts* (strict + one retry
each), still bounded and non-amplifying (no repair loop). See
[Security model](#security-model) R2.

## Configuration

**None.** The trailing-comma fallback is always on and has no environment
variable, CLI flag, or config key. It is a structural, fail-closed recovery of a
specific defect, not a tunable leniency mode — there is nothing safe to disable
and nothing dangerous to enable. Because it is fallback-only and removal-only,
it cannot change the outcome of any input that already parses or of any input
that is malformed beyond a single structural trailing comma.

This is intentionally **not** a general JSON-repair or JSON5 mode: no new
dependency is added, the exact-pinned `serde` / `serde_json` versions are
unchanged, and no `json5` / `json_repair` crate is introduced. The primitive is
`std`-only.

## Telemetry

No telemetry code changed. The existing
[`record_parse_outcome("distill", parsed.is_ok())`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/mod.rs)
calls key off whether a parse succeeded; the retry simply flips outcomes that
were `false` (strict miss on a trailing-comma object) to `true`. The observable
effects:

- `recipe_parse_success_total{phase=distill}` rises; the correlated
  `recipe_parse_failure_total{phase=distill}` falls.
- `distill_parse_success_rate` climbs off `0` toward `1.0`.
- The Overseer stops emitting `anomaly:distill parse-fail rate 100%`.

The `(phase, count-only)` metric schema is unchanged and **no raw payload is
logged** — the retry path adds no content logging, so there is no new
log-injection or PII surface. To watch the rate recover:

```bash
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
  | tail -n 50
```

If, after a real trailing-comma sample is confirmed recovered, the quality gym
gate still reports `gym_skipped`, that is a **new** finding to escalate under
the [Overseer root-cause (WHY) principle](../concepts/overseer-root-cause-why.md)
— the gym skip is treated here only as a downstream side-effect of the upstream
distill failure, not an independent defect.

## Security model

The trust boundary is: untrusted recipe-subprocess / LLM output →
`strip_json_trailing_commas` → `serde_json` → `RecipeEnvelope` → persisted
cognitive memory. The change is fully in-process; it adds no network, no I/O, no
credentials, and no config surface.

| # | Property | How it is guaranteed |
| --- | --- | --- |
| R1 | **Infallible / no panic** | Total byte-scan over `as_bytes()`; no boundary-splitting index; verified by empty / huge / lone-`\` / all-comma / non-ASCII negative tests. A malformed or adversarial payload can never crash the distill loop. |
| R2 | **Bounded work** | Strictly O(n) single pass over the bytes handed to the primitive, no repair-until-stable loop — no algorithmic-complexity DoS of the distill loop. On the slow path the retry runs per *disjoint* balanced span, so total stripper work stays linear in the cleaned text; an adversarial many-span input at most doubles parse *attempts* (strict + one retry each), still bounded. |
| R3 | **UTF-8 integrity** | Only the ASCII `,` (`0x2C`) is removed; result rebuilt with checked `String::from_utf8`. No `unsafe`, no `from_utf8_unchecked`. |
| R4 | **String-literal awareness** | Commas inside `"…"` (honouring `\` escapes) are never removed, so fact content is never corrupted. |
| R5 | **Fail-closed integrity** | Fallback-only, structural-only, single retry. Malformed input still yields `Err` → `None` → batch deferred. No hollow `Ok` — prevents memory poisoning via over-lenient parsing. |
| R6 | **Removal-only** | Output bytes ⊆ input bytes; the primitive cannot inject a fact the model did not emit. |
| R7 | **No content in logs** | The retry adds no raw-span logging; telemetry stays `(phase, count-only)`. |

Hermetic: no new crates, no I/O, no env / config / credentials.

## Examples

The examples below show the observable input → outcome contract. They are not a
public CLI; the primitive is an internal parsing helper.

### A trailing-comma facts object is recovered

The distill agent emits a well-intentioned object with one trailing comma:

```json
{
  "facts": [
    { "concept": "kgpacks-rs ships int8 PQ embeddings", "source_episode_id": "ep-8842" }
  ],
}
```

Strict `serde_json` rejects it (trailing `,` before `}`). The fallback strips
the structural comma, the retry parses, and the pass recovers the fact —
`distill_parse_success_rate` moves off `0`.

### A comma inside a string is preserved

```json
{ "facts": [ { "concept": "supports x, y, and z", "source_episode_id": "ep-3" } ], }
```

Only the final structural `,` (before the outer `}`) is removed. The commas
inside `"supports x, y, and z"` are untouched, so the fact text is byte-identical
after recovery.

### A genuinely malformed object still fails closed

```json
{ "facts":
```

No structural trailing comma is present to remove; the comma-stripped view is
identical, the retry misses too, `parse_recipe_envelope` returns `None`,
`parse_facts_document` returns `Err`, and the batch is deferred — no hollow
success is persisted. An object with an **adjacent** comma run such as `[1,,]`
likewise stays malformed — the single pass removes only the last comma (leaving
`[1,]`) and does not revisit it — so it stays `Err`. (A *non-adjacent* multi-comma
object such as `{ "facts": [ … ], }` is fully recovered, because both structural
commas are handled independently in the one pass; see
[What counts as a structural trailing comma](#what-counts-as-a-structural-trailing-comma).)

### Well-formed output is byte-identical (no-op proof)

```json
{ "facts": [], "procedures": [] }
```

Strict parse succeeds on the first attempt; the fallback never runs. Even if
`strip_json_trailing_commas` were invoked on this text it would return
`Cow::Borrowed` — the exact input, zero allocation.

## When the fallback does *not* fire

The retry is intentionally narrow. It does nothing (and the result is identical
to the pre-#2658 behavior) when:

- Strict `serde_json` already accepts the span — the common case; the fallback
  is never reached.
- The defect is **not** a structural trailing comma — e.g. a missing brace, an
  unquoted key, a comment, an **adjacent** double comma (`,,`), or truncated
  JSON. There is nothing for the removal-only, single-pass primitive to fully
  fix, so the span stays `Err` and the batch is deferred. (A *non-adjacent*
  run of structural trailing commas is not in this list — it is recovered.)
- The span is empty after noise-stripping — the empty-document guard in
  `parse_facts_document` returns `Err` before any parse.

This keeps the change scoped to exactly the one defect that produced the
recurring `100%` anomaly, and leaves every other failure mode — including the
ones the [raw-capture diagnostic](./distill-raw-capture-on-parse-failure.md)
exists to harvest — behaving as before.

## Tests

Regression coverage lives inline (`#[cfg(test)]`) next to the code.

**Unit tests — `src/recipe_output/extract.rs`:**

- `{"facts":[],}`, `[1,2,]`, and newline-separated `,\n}` → structural comma
  removed and the result is `serde_json`-accepted.
- **Non-adjacent multi-comma** `{ "facts": [ {…}, ], }` (comma after the last
  array element *and* after the last object member) → both structural commas
  removed in the one pass; the result is `serde_json`-accepted.
- In-string comma `{"a":"x,y","b":1,}` → only the structural comma is removed;
  `"x,y"` is preserved.
- Escaped-quote content `{"a":"x\",",}` → the comma inside the string is
  preserved; only the structural one is removed.
- Clean `{"a":1}` → `matches!(result, Cow::Borrowed(_))` (no-op proof).
- Adjacent double comma `[1,,]` → still `Err` (only the last comma is removed
  in a single pass; the first is not revisited).
- Security / robustness: empty input, a very large input, a lone trailing `\`,
  an all-comma string, and non-ASCII bytes adjacent to a structural comma → no
  panic, no false `Ok`.

**Integration tests — `src/memory_consolidation/distillation.rs`:**

- A trailing-comma `{ "facts": [...] }` document recovers ≥ 1 fact through
  `parse_facts_document`.
- A trailing-comma object carrying a grounded `source_episode_id` still wins the
  grounded-capable tier over a later source-less object.
- A genuinely malformed `{ "facts":` document still returns `Err`.
- The existing clean-path `parse_recipe_output_*` / envelope tests remain green
  (fallback never runs on well-formed input).

Run them with:

```bash
cargo test memory_consolidation::distillation
cargo test recipe_output
cargo fmt --check
cargo clippy
```

## Related

- [Distill recipe output capture](./distill-recipe-output-capture.md) — the
  `RecipeEnvelope` / `parse_facts_document` contract this feature relaxes on a
  strict miss.
- [Distill raw-capture on parse failure](./distill-raw-capture-on-parse-failure.md)
  — the env-gated diagnostic for harvesting a *residual* failing sample once the
  trailing-comma class is recovered.
- [Text-parsing wire formats](./text-parsing-wire-formats.md) — the normative
  catalogue of recipe/LLM wire formats and the shared `recipe_output`
  pre-stripping chokepoint.
- [Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md)
  — the noise-stripping that runs before this parse.
- [Overseer root-cause (WHY) principle](../concepts/overseer-root-cause-why.md)
  — why the recurring anomaly signature is traced to a root cause (this fix)
  rather than symptom-patched each cycle.
- [Episode distillation](../architecture/episode-distillation.md) — the
  surrounding pipeline.
