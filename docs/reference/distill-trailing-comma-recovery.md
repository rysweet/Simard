---
title: Distill trailing-comma parse recovery
description: Reference for the string-aware trailing-comma JSON recovery that restores distillation's learning loop when the distill agent emits otherwise-valid JSON with a trailing comma before a closing brace/bracket — the strip_json_trailing_commas primitive, its Cow::Owned "recovery happened" discriminant, the recover-only-on-Owned wiring into scan_cleaned_for_facts, the distinct zero-facts warn that keeps parse-fail and yield-loss telemetrically separate, and the distill_parse_success_rate metric it drives back to 1.0.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./distill-recipe-output-capture.md
  - ./distill-raw-capture-on-parse-failure.md
  - ./text-parsing-wire-formats.md
  - ./telemetry-metrics.md
  - ../architecture/episode-distillation.md
  - ../howto/recover-from-distill-trailing-comma-parse-failures.md
  - ../concepts/copilot-launcher-preamble-stripping.md
---

# Reference: Distill trailing-comma parse recovery

Crate: `simard`

> **Status — implements the
> [#2658](https://github.com/rysweet/Simard/issues/2658) fix and clears the
> recurring [#2672](https://github.com/rysweet/Simard/issues/2672)
> `overseer-obs:anomaly:distill parse-fail rate 100%` signature.** Present
> tense below describes shipped behavior. Locations:
> primitive + unit tests `src/recipe_output/extract.rs`;
> re-export `src/recipe_output/mod.rs`;
> recovery wiring + zero-facts warn + regression tests
> `src/memory_consolidation/distillation.rs`.

The episode-distillation pass turns batches of episodic memory into semantic
**facts** and reusable **procedures** by shelling out to `recipe-runner-rs` and
parsing the distill agent's `{ "facts": [...], "procedures": [...] }` object
(see [Distill recipe output capture](./distill-recipe-output-capture.md)).

The final acceptance gate for that object is strict `serde_json`. That is
correct — widening syntax tolerance must never widen *trust*. But one very
common LLM surface defect made strict parsing reject an answer that was
otherwise perfectly well-formed: a **trailing comma** before a closing `}` or
`]`:

```json
{ "facts": [ {"concept": "bug-pattern", "content": "…", "source_episode_id": "t=42"}, ] }
```

`serde_json` (JSON, not JSON5) rejects the `,]`. Every span the distiller
emitted this way failed strict parsing, so `scan_cleaned_for_facts` returned
`None`, the pass fell to a Tier-3 `Err` (deferred batch), and **no facts were
stored**. Repeated across every pass, `distill_parse_success_rate` sat at
`0.0`, the Overseer observed `anomaly:distill parse-fail rate 100%`, the
learning loop was dead, and the umbrella parity goals starved
(`goal:blocked:*`, `process:distill_fail`, `quality:gym_skipped`).

This feature adds a **bounded, delete-only, string-aware** trailing-comma
recovery that runs **only after** strict parsing has already missed, and
**only when** a structural comma was actually removable. Strict `serde_json`
remains the sole acceptance gate; recovery widens the syntax Simard *tolerates*
without widening what she *trusts*.

---

## The primitive: `strip_json_trailing_commas`

```rust
use std::borrow::Cow;

/// Delete every *structural* trailing comma — a `,` whose next non-whitespace
/// byte is `}` or `]` — from `s`, honouring JSON string literals so a comma
/// inside a `"…"` value is never touched.
///
/// Single-pass, delete-only. Returns [`Cow::Borrowed`] (byte-identical,
/// zero-allocation) when there is nothing to remove — the clean path. Returns
/// [`Cow::Owned`] **only** when at least one comma was actually removed, so the
/// `Owned` variant is itself the "a trailing comma was present and stripped"
/// discriminant callers branch on.
///
/// Output length is always `<= input length` (delete-only; never rewrites,
/// escapes, or inserts).
pub fn strip_json_trailing_commas(s: &str) -> Cow<'_, str>
```

Location: `src/recipe_output/extract.rs`. Re-exported from
`src/recipe_output/mod.rs` so callers use
`crate::recipe_output::strip_json_trailing_commas`, alongside the existing
`balanced_objects`, `last_balanced_object`, `strip_ansi`, and
`strip_recipe_noise` primitives.

### State machine

The scan mirrors the `in_string` / `escaped` logic of the existing
`scan_balanced` helper in the same module, so string-literal handling is
identical to (and tested against) the balanced-brace scanner distillation
already relies on:

| State | Byte | Action |
|---|---|---|
| outside string | `"` | enter string literal |
| outside string | `,` then (skipping whitespace) `}` or `]` | **drop the `,`** (recovery) |
| outside string | anything else | copy through |
| inside string | `\` | mark next byte escaped (copy through) |
| inside string | `"` (unescaped) | leave string literal |
| inside string | anything else (incl. `,`, `}`, `]`) | copy through **untouched** |

Because the comma is only removed when it is **outside** a string literal and
the **next non-whitespace byte closes a container**, a comma that is legitimate
JSON data — inside a string value, or separating two elements — is never
removed. A `,` inside a string such as `"a, b,"` is copied verbatim.

### Clean-path guarantee

Input with no structural trailing comma passes through **byte-identical and
zero-allocation** as `Cow::Borrowed`, mirroring the `strip_ansi` /
`strip_recipe_noise` precedent. Adopting recovery therefore changes **nothing**
for well-formed output — the overwhelmingly common case.

---

## Recovery wiring: `scan_cleaned_for_facts`

Recovery is invoked from `scan_cleaned_for_facts`
(`src/memory_consolidation/distillation.rs`), the function that turns cleaned
recipe text into a `DistillOutput`. The order of attempts is unchanged except
for one **appended** recovery stage that runs last:

1. **Strict fast path.** `serde_json::from_str::<RecipeEnvelope>(trimmed)` on
   the whole cleaned text. On success, return. *(unchanged)*
2. **Strict slow path.** For each string-aware `balanced_objects(trimmed)` span,
   scanned from the end, try strict `serde_json::from_str::<RecipeEnvelope>`,
   applying the existing three preference tiers (grounded-capable fact →
   otherwise-non-empty → empty `{"facts":[]}`). *(unchanged)*
3. **Trailing-comma recovery (new).** Only if steps 1–2 all missed:
   `let cleaned = strip_json_trailing_commas(trimmed);`
   - **Recover only on `Cow::Owned`.** If `cleaned` is `Cow::Borrowed`, the text
     had no structural trailing comma, so recovery cannot possibly change the
     parse outcome — return `None` immediately (no redundant re-parse). This is
     the guard that keeps recovery from ever masking a *different* malformed
     input as success.
   - If `cleaned` is `Cow::Owned`, re-run the **same two strict stages** on the
     cleaned text: fast path on `&cleaned`, then per-span slow path over
     `balanced_objects(&cleaned)` with the identical tier logic.
4. If every stage above yields nothing, return `None`.

The hard invariant is preserved: **`None` → `Err` → deferred batch.** Genuinely
malformed JSON (unbalanced braces, missing quotes, truncated output) is not a
trailing-comma defect, so `strip_json_trailing_commas` either returns
`Cow::Borrowed` (nothing removed → skip) or returns cleaned text that still
fails strict `serde_json` (→ `None`). Either way the batch is deferred and
retried, never silently dropped and never returned as a hollow `Ok`.

### What recovery does **not** change

- The `success == false` short-circuit upstream is untouched.
- The concept allow-list in `RecipeEnvelope::into_facts` — only
  `pr-pattern`, `bug-pattern`, `lesson-learned` survive
  `canonical_distill_concept` — is untouched. Recovery widens accepted
  **syntax**, never accepted **content**.
- Downstream reliability quarantine (`assess_fact_reliability`) and provenance
  gating are untouched.

---

## Failure-mode disambiguation: the zero-facts warn

Two distinct distillation failure modes must never be conflated, because they
demand opposite fixes:

| Mode | Meaning | Signal | Fix direction |
|---|---|---|---|
| **Parse-fail** | Output never became a `RecipeEnvelope` | `scan_cleaned_for_facts` → `None` → `Err` (deferred) | extractor / syntax tolerance (this feature) |
| **Zero-facts** | Output parsed fine, but **every** fact was dropped by the allow-list | `Ok` with an empty kept `Vec` + a distinct `warn` | prompt / concept-label side |

Yield-loss detection is a **pass-level** check, run **once per pass** at the
single point where `scan_cleaned_for_facts` returns its finally-selected
`DistillOutput`. It compares the **selected** candidate's raw input `facts`
count against the facts that survived the allow-list, and emits one dedicated
`tracing::warn` on target `simard::distill` **iff** that selected parse had a
non-empty `facts` array but **zero** facts survived `canonical_distill_concept`:

```text
WARN simard::distill valid distill parse yielded zero allow-listed facts
     input_concepts=3 kept_facts=0
```

**Why pass-level, not per-envelope.** `scan_cleaned_for_facts` may parse
*several* candidate objects in a single pass — the strict fast path plus every
balanced span in the reverse scan — and calls `into_output()` (hence
`into_facts()`) on each. Emitting the warn inside `into_facts` would fire once
per parsed candidate and, worse, warn for an early empty candidate even when a
*later* span wins with grounded facts — conflating a **successful** pass with
yield-loss and defeating the disambiguation this warn exists for. Anchoring the
check to the **selected** output instead guarantees:

- **At most one warn per pass** — low-cardinality logging is preserved.
- **A pass that keeps ≥ 1 fact never warns.** Losing candidates in the tier
  scan are silent; success and yield-loss can never be read as the same
  incident.

`RecipeEnvelope::into_facts` therefore stays a **pure filter**: it maps and
drops facts by the allow-list and emits nothing itself.

The warn carries **only** counts (`input_concepts`, `kept_facts=0`) and a
failure class — never fact content, never `source_episode_id`, never the raw
payload. This makes the two modes machine-distinguishable in the logs: a
parse-fail is an `Err` on the deferral path; a yield-loss is a single `Ok`
pass plus this one warn. They can no longer be read as the same incident.

---

## Telemetry: `distill_parse_success_rate`

The parse-success metric is **not** new — it ships as of
[#2512](https://github.com/rysweet/Simard/issues/2512) via
`record_distill_success_metric` in `distillation.rs`, which emits a
`distill_parse_success_rate` event (value `1.0` / `0.0`) once per pass that
**reached output parsing**. No-op under `cfg!(test)`. See
[Telemetry metrics](./telemetry-metrics.md).

This feature does not add a metric; it **drives the existing one back to
`1.0`** for trailing-comma inputs that previously scored `0.0`. When the metric
recovers above the anomaly threshold, the Overseer stops emitting
`anomaly:distill parse-fail rate 100%`. The regression suite asserts this
directly: a trailing-comma batch that formerly scored `0.0` now scores `1.0`.

| Signal | Emitted by | Meaning after this fix |
|---|---|---|
| `distill_parse_success_rate` | `record_distill_success_metric` (`distillation.rs`) | mean rises to `1.0` for trailing-comma batches; anomaly clears |
| `WARN simard::distill … kept_facts=0` | pass-level check in `scan_cleaned_for_facts` (on the selected output) | valid-parse-yields-zero-facts (yield-loss), distinct from parse-fail |

---

## Types (existing, unchanged)

```rust
#[derive(serde::Deserialize)]
struct RecipeEnvelope {
    facts: Vec<RecipeFact>,
    #[serde(default)]
    procedures: Vec<RecipeProcedure>,
}

// Canonicalises an LLM concept label to one of exactly three allow-listed
// values, tolerating case / whitespace / `_`↔`-` surface variation; returns
// `None` (fact dropped) for anything off-spec.
pub(crate) fn canonical_distill_concept(raw: &str) -> Option<&'static str>;
// → Some("pr-pattern") | Some("bug-pattern") | Some("lesson-learned") | None
```

Recovery operates entirely on the **text before** deserialization, so no
type or schema changes are required.

---

## Regression tests

Located in the `#[cfg(test)]` modules of `src/recipe_output/extract.rs`
(primitive units, `issue_2672_trailing_comma_tests`) and
`src/memory_consolidation/distillation.rs` (recovery + yield-loss integration
tests T1–T6, `issue_2672_trailing_comma_recovery_tests`).

Primitive units (`extract.rs`):

- Clean input (no trailing comma) → `Cow::Borrowed`, byte-identical.
- `{"a":1,}` → `Cow::Owned`, `{"a":1}`.
- `[1,2,]` → `Cow::Owned`, `[1,2]`.
- `"a, b,"` (comma inside a string) → `Cow::Borrowed`, untouched.
- Output length always `<= input length`.

Recovery/integration tests (`distillation.rs`):

| Test | Input | Expected |
|---|---|---|
| **T1** | bare `{"facts":[ … ],}` trailing comma | parses; ≥ 1 fact |
| **T2** | enveloped / prose-wrapped trailing-comma payload | recovers; ≥ 1 fact |
| **T3** | structural trailing comma **and** a comma inside a string value | recovers; the in-string comma is preserved verbatim (no corruption) |
| **T4** | genuinely malformed JSON | still `None`/`Err` (deferral preserved, no hollow `Ok`) |
| **T5** | valid parse, non-empty facts, **zero** allow-listed concepts | `Ok` empty **+ exactly one warn**, not `Err` |
| **T6** | empty candidate followed by a later winning grounded span | grounded fact kept; **no warn** (success is never conflated with yield-loss) |
| **B1** | trailing-comma batch | `distill_parse_success_rate` reaches `1.0` |

T3 exercises the recovery path *and* its string-awareness together; the
focused in-string guard for the primitive alone is the `strip_json_trailing_commas`
unit (`"a, b,"` → `Cow::Borrowed`, untouched) listed above.

---

## Security model

- **Untrusted input, treated as hostile.** The distill agent's stdout is
  untrusted LLM subprocess output.
- **Delete-only recovery.** No regex, no recursion, no `json5`/`eval`, no
  external deserializer. Output length is always `<= input length`.
- **String-aware.** Bytes inside string literals are never mutated; only
  structural commas outside strings are removed.
- **Strict serde remains the only acceptance gate.** Widening syntax tolerance
  never widens trust; ungrounded / off-spec facts are still quarantined
  downstream by the allow-list and reliability gate.
- **Telemetry carries counts only.** The yield-loss warn emits via
  `tracing::warn` (target `simard::distill`); the `distill_parse_success_rate`
  metric emits via the existing atomic `self_metrics::record_metric` append.
  Both carry only counts and a failure-class label — never fact content,
  `source_episode_id`, or raw payload; no new files, paths, or permissions.
- **Deferred-batch integrity preserved.** Any input that cannot be strictly
  parsed after bounded recovery persists as `Err` and is retried, never
  silently dropped.

---

## Related

- [Distill recipe output capture](./distill-recipe-output-capture.md) — the
  output-capture contract and the three-tier parser this stage extends.
- [Distill raw-capture on parse failure](./distill-raw-capture-on-parse-failure.md)
  — harvest the exact bytes of a *residual* (non-trailing-comma) parse failure.
- [Text-parsing wire formats](./text-parsing-wire-formats.md) — the shared
  `recipe_output` noise-stripping primitives (`strip_ansi`, `balanced_objects`)
  that recovery mirrors and composes with.
- [Recover from distill trailing-comma parse failures](../howto/recover-from-distill-trailing-comma-parse-failures.md)
  — the operator runbook for the `anomaly:distill parse-fail rate 100%` signal.
- [Episode distillation](../architecture/episode-distillation.md) — the
  surrounding pipeline.
