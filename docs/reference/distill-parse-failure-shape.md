---
title: Distill parse-failure shape classification
description: Reference for the measure-first sub-classification of a distill ParseFailure into MissingFile / EmptyDocument / UnparseableObject and the evidence-gated lenient JSON recovery it drives — the ParseFailureShape enum and prefix-anchored classify_parse_failure_shape() co-located with classify_distill_error in memory_consolidation::distillation, the generic strip_json_<defect> transform family in recipe_output::extract chained into parse_facts_envelope_lenient, the parse_failure_shape enrichment of the existing distill_parse_success_rate metric context, and the invariants (strict-parse-first, provable no-op on valid JSON, grounding gate authoritative, Err-never-marks-episodes) that keep precision and longitudinal metrics intact.
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
  - ../howto/capture-and-diagnose-a-failing-distill-sample.md
  - ../../src/recipe_output/extract.rs
  - ../../src/recipe_output/mod.rs
  - ../../src/memory_consolidation/distillation.rs
---

# Distill parse-failure shape classification

> **Status: implemented — issue
> [#2495](https://github.com/rysweet/Simard/issues/2495) (distill parse-failure
> rate).** Present tense below describes the shipped behavior. Locations: the
> The `ParseFailureShape` enum + `classify_parse_failure_shape()` classifier live
> beside `classify_distill_error`, the metric enrichment, and the lenient-parse
> ladder in
> [`src/memory_consolidation/distillation.rs`](https://github.com/rysweet/Simard/blob/main/src/memory_consolidation/distillation.rs).
> The generic `strip_json_*` transforms live in
> [`src/recipe_output/extract.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/extract.rs)
> and are re-exported from
> [`src/recipe_output/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/mod.rs).
> Tests are inline `#[cfg(test)]` in both files and in
> `distillation_tests.rs`.

When a distillation pass classifies as a
[`ParseFailure`](./distill-recipe-output-capture.md) — the recipe exited `0` and
the agent ran, but its facts document yielded no usable
`{ "facts": [...] }` object — the failure is non-fatal by design: no episodes are
marked and the batch retries. That resilience is correct, but a bare
`parse-failure` counter cannot tell you **which** of three very different things
went wrong, and only one of them is fixable by the parser at all. This page
documents the sub-classification that answers that question, the
**evidence-gated** lenient JSON recovery it unlocks, and the invariants that keep
both from weakening precision or breaking longitudinal metrics.

This is the measurement-and-recovery layer that sits *inside* the existing
`ParseFailure` bucket defined in
[Distill recipe output capture](./distill-recipe-output-capture.md). It does not
change the `ParseFailure` denominator, the metric names, or the
`Err`-never-marks-episodes contract; it enriches them. For the raw-byte
harvesting diagnostic that feeds the evidence used here, see
[Distill raw-capture on parse failure](./distill-raw-capture-on-parse-failure.md).

## Contents

- [Why sub-classify a parse failure](#why-sub-classify-a-parse-failure)
- [The three parse-failure shapes](#the-three-parse-failure-shapes)
- [Measure first, fix second](#measure-first-fix-second)
- [Public API](#public-api)
  - [`ParseFailureShape`](#parsefailureshape)
  - [`classify_parse_failure_shape`](#classify_parse_failure_shape)
  - [The `strip_json_*` transform family](#the-strip_json_-transform-family)
  - [`parse_facts_envelope_lenient`](#parse_facts_envelope_lenient)
- [Metric enrichment](#metric-enrichment)
- [Configuration](#configuration)
- [Transform authoring contract](#transform-authoring-contract)
- [Invariants preserved](#invariants-preserved)
- [Security model](#security-model)
- [Examples](#examples)
- [When shape classification does *not* apply](#when-shape-classification-does-not-apply)
- [Related](#related)

## Why sub-classify a parse failure

The historical "≈80% distill parse-failure rate" that motivated
[#2495](https://github.com/rysweet/Simard/issues/2495) predates the shared-
chokepoint hardening (`recipe_output::extract`), the dedicated facts-file channel
(#2622/#2619), and the trailing-comma recovery (#2658). By the time those landed,
the residual `parse-failure` events had **three distinct root causes hiding
behind one label**:

1. the agent never wrote its facts file at all,
2. the agent wrote an **empty** file, or
3. the agent wrote a file whose bytes are *almost* JSON but do not strictly
   parse.

Only case (3) is addressable by the parser. Cases (1) and (2) are **source-side**
— the fix is prompt/retry tuning, not leniency — and no amount of parser work can
recover facts that were never emitted. Spending parser effort on a residual that
is dominated by (1)/(2) would be waste at best and a precision risk at worst.
Sub-classification makes the split measurable so the fix targets the layer that
is actually broken, exactly like [raw-capture](./distill-raw-capture-on-parse-failure.md)
made the failing bytes inspectable.

## The three parse-failure shapes

Every `DistillFailureClass::ParseFailure` (see
[Distill recipe output capture](./distill-recipe-output-capture.md)) carries
exactly one shape, discriminated from the **stable leading prefix** of the
`SimardError::RpcError` message the distill parser emits at each site — the same
prefix-anchored discipline as
[`classify_distill_error`](./distill-recipe-output-capture.md), never a `contains`
scan (the messages embed a variable, truncated document excerpt in their tail).

| Shape | Error prefix (anchor) | Source of the failure | Parser-addressable? |
| --- | --- | --- | --- |
| `MissingFile` | `distill: facts output file was not written` | Agent produced no facts file. | **No** — source-side (prompt/retry). |
| `EmptyDocument` | `distill: facts document was empty` | File exists but is whitespace-only. | **No** — source-side (prompt/retry). |
| `UnparseableObject` | `distill: facts document did not contain a parseable` | Bytes were written but no balanced `{ "facts": … }` object strictly parses. | **Yes** — lenient recovery may apply. |

`MissingFile` and `EmptyDocument` are terminal for the parser: there is nothing to
recover. `UnparseableObject` is the only shape the `strip_json_*` transforms below
can convert into a success.

## Measure first, fix second

The feature shipped in two gated stages, and the ordering is load-bearing:

- **S1 — measure only.** `ParseFailureShape` + `classify_parse_failure_shape()`
  ship first and enrich the existing `distill_parse_success_rate` metric
  `context` with the shape label. No parser behavior changes. This is
  independently shippable and de-risks everything downstream — it turns "≈80%,
  historical" into a **measured** A/B/C split against the current parser.
- **Gate.** If `MissingFile` / `EmptyDocument` dominate the measured split, parser
  work stops here — the residual is unfixable by any transform, and the follow-up
  is prompt/retry tuning. Lenient transforms ship **only** when
  `UnparseableObject` is a material share.
- **S2 — evidence-gated recovery.** For each real `UnparseableObject` defect
  observed in harvested samples, one `strip_json_<defect>` transform is added to
  `recipe_output::extract` and chained into the lenient parse ladder. Every
  transform is driven by a real sample, never a hypothetical.

The acceptance target is defined against the **measured** current
`distill_parse_success_rate`, not the stale 80% figure: parse-success ≥ 90%
(parse-failure ≤ 10%) for the `UnparseableObject`-addressable share.

## Public API

The API is split across two modules by ownership (see the design note below).

The generic, phase-agnostic JSON transforms live in `recipe_output::extract` and
are re-exported from `recipe_output` alongside the existing `balanced_objects`,
`extract_json_payload`, and `strip_json_trailing_commas`:

```rust
// recipe_output::mod — shared transforms, reused by every recipe-backed phase.
pub use extract::{
    // pre-existing …
    balanced_objects, extract_json_payload, last_balanced_object,
    strip_ansi, strip_json_trailing_commas, strip_recipe_noise,
    // evidence-gated lenient transforms (#2495) — the shipped set …
    strip_json_code_fences,
};
```

The distill-specific classification lives in `memory_consolidation::distillation`
as `pub` items beside `classify_distill_error` / `DistillFailureClass`:

```rust
// memory_consolidation::distillation
pub enum ParseFailureShape { /* … */ }
pub fn classify_parse_failure_shape(err: &SimardError) -> Option<ParseFailureShape>;
```

> **Design note — module placement (resolved).** Placement follows a strict
> layering rule. The `strip_json_*` transforms are pure, phase-agnostic JSON
> surgery and live in the shared `recipe_output::extract` chokepoint, so a vetted
> transform benefits every recipe-backed phase (brain, OODA, merge-judge), not
> just distill. `classify_parse_failure_shape`, by contrast, anchors on
> **distill-owned** message prefixes (`distill: facts document …`) whose sibling
> `classify_distill_error` lives in `memory_consolidation::distillation`; it is
> therefore **co-located with `classify_distill_error` + `ParseFailureShape` in
> `distillation`**, not in the shared module. Hard-coding those distill-private
> strings into a phase-agnostic module would couple a shared primitive to one
> consumer's error contract — a layering inversion. Keeping the classifier in
> `distillation` makes the "mirror `classify_distill_error`, one level deeper"
> intent literal and leaves `recipe_output::extract` free of distill-specific
> strings.

### `ParseFailureShape`

A closed enum with stable, snake-case string labels. `Copy`, so it is threaded
through the metric-context builder by value like `DistillFailureClass`.

```rust
/// The sub-shape of a distill `ParseFailure`. Closed set: only these three
/// shapes are possible, and only `UnparseableObject` is parser-addressable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseFailureShape {
    /// The agent's facts file was never written (source-side).
    MissingFile,
    /// The facts file exists but is empty/whitespace-only (source-side).
    EmptyDocument,
    /// Bytes were written but no balanced `{ "facts": … }` object strictly
    /// parses (the only parser-addressable shape).
    UnparseableObject,
}

impl ParseFailureShape {
    /// Stable label for the metric `context` and the raw-capture header.
    /// Never the raw payload — labels only.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingFile => "missing-file",
            Self::EmptyDocument => "empty-document",
            Self::UnparseableObject => "unparseable-object",
        }
    }

    /// `true` only for the shape a `strip_json_*` transform can recover.
    pub fn is_parser_addressable(self) -> bool {
        matches!(self, Self::UnparseableObject)
    }
}
```

The labels are stable wire values: they appear in `metrics.jsonl` and in
harvested raw-capture headers, so tooling and longitudinal queries key off them.
Renaming a label is a breaking change.

### `classify_parse_failure_shape`

A **pure**, allocation-free classifier. It anchors on the leading prefix of the
distill parse-failure message and returns the shape — mirroring
`classify_distill_error` one level deeper. It returns `Some(shape)` **only** for a
message that `classify_distill_error` would bucket as `ParseFailure`; any other
message returns `None` (never a shape), so an unexpected or non-`ParseFailure`
error can never be mislabeled — and, critically, can never be mislabeled as the
one *parser-addressable* shape and thereby falsely invite transform work. `None`
maps to the same `null` `parse_failure_shape` the metric already records outside
`ParseFailure`, so the two rules stay identical and the A/B/C split is never
polluted by a defensive fall-through.

```rust
/// Sub-classify a distill `ParseFailure` into its [`ParseFailureShape`].
///
/// Prefix-anchored on the stable `SimardError::RpcError` message the distill
/// parser emits, NOT `contains` (the message tail carries a truncated document
/// excerpt). Returns `None` for any message that is not one of the three
/// `ParseFailure` prefixes. Pure and panic-free.
pub fn classify_parse_failure_shape(err: &SimardError) -> Option<ParseFailureShape>;
```

Example:

```rust
use simard::memory_consolidation::distillation::{classify_parse_failure_shape, ParseFailureShape};

let empty = SimardError::RpcError(
    "distill: facts document was empty; the agent produced no output".into(),
);
assert_eq!(classify_parse_failure_shape(&empty), Some(ParseFailureShape::EmptyDocument));
assert!(!ParseFailureShape::EmptyDocument.is_parser_addressable());
```

### The `strip_json_*` transform family

Each transform is a last-resort **recovery view** over a candidate balanced
object, added only after S1 evidence shows the corresponding defect in real
samples. Every transform obeys one signature and one hard contract:

```rust
/// A lenient recovery transform: `fn(&str) -> Cow<'_, str>`.
///
/// Contract (enforced by tests):
///   * **Provable no-op on valid JSON** — returns `Cow::Borrowed` byte-for-byte
///     when the defect is absent (the zero-allocation clean path). Giving up is
///     `Cow::Borrowed`, never a lossy rewrite.
///   * **String-literal aware** — a defect byte inside a JSON string (respecting
///     `\"` escapes) is never touched, so a fact's `content` is preserved
///     verbatim.
///   * **Single-pass, O(n), no regex, no input-depth recursion** — bounded work
///     on adversarial input.
///   * **Never widens acceptance** — a genuinely malformed object is left
///     malformed so the caller's strict parse still rejects it.
pub fn strip_json_trailing_commas(s: &str) -> Cow<'_, str>; // pre-existing (#2658)
pub fn strip_json_code_fences(s: &str) -> Cow<'_, str>;     // #2495, shipped set
```

`strip_json_trailing_commas` (from #2658) removes a single trailing `,` before a
`}`/`]`. `strip_json_code_fences` removes a Markdown code fence
(```` ```json `` … `` ``` ````) that wraps an otherwise-valid object, which some
agents emit around their answer. The family is **extensible by the same
contract**: a new `strip_json_<defect>` is added per observed
`UnparseableObject` defect class and chained (below). No JSON5 crate, no
`serde_json::Value`/flatten widening, no new dependency — the transforms are
hand-rolled and the target stays the closed `RecipeEnvelope` type.

### `parse_facts_envelope_lenient`

The distill parse ladder tries strict `serde_json` **first** (so the clean path is
byte-identical and unchanged), then each vetted transform in turn, attempting a
transform only when the strict parse still fails and only when the transform
actually changed the bytes (an `Owned` result — a `Borrowed` result means the
strict parse already saw those exact bytes):

```rust
fn parse_facts_envelope_lenient(text: &str) -> Option<RecipeEnvelope> {
    // 1. Strict first — clean path, zero leniency.
    if let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(text) {
        return Some(parsed);
    }
    // 2. Chain the recovery views. A wrapping code fence is unwrapped first, then
    //    the (possibly unwrapped) body is trailing-comma-stripped, so a payload
    //    carrying BOTH defects still recovers. Only an actually-transformed
    //    (`Owned`) view is worth re-parsing; `Borrowed` means "no defect, nothing
    //    new to try".
    let unfenced = strip_json_code_fences(text);
    if let Cow::Owned(body) = &unfenced
        && let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(body)
    {
        return Some(parsed);
    }
    if let Cow::Owned(stripped) = strip_json_trailing_commas(&unfenced)
        && let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(&stripped)
    {
        return Some(parsed);
    }
    None
}
```

This function is the single chokepoint the fast path and the
`balanced_objects` slow path in `scan_cleaned_for_facts` both call, so the
grounding/preference tiers (grounded-capable object > non-empty > empty) that
select *which* object wins are unchanged and remain authoritative.

## Metric enrichment

Sub-classification adds **one field** to the existing
`distill_parse_success_rate` context (and to the `distill_success_rate` context
it shares). It does **not** add a new metric name, change the denominator gate
(`parse_attempted`), or alter the `value` semantics — so every existing
longitudinal query keeps working and the mean of `distill_parse_success_rate`
remains exactly the parse-success rate.

| Field | Type | Meaning |
| --- | --- | --- |
| `parse_failure_shape` | string \| null | `null` on success and for any non-`ParseFailure` class; otherwise `missing-file` \| `empty-document` \| `unparseable-object`. |

The full context (unchanged fields plus the new one):

```json
{
  "outcome": "failure",
  "recipe_exited_ok": true,
  "parse_attempted": true,
  "parse_success": false,
  "failure_class": "parse-failure",
  "parse_failure_shape": "unparseable-object",
  "input_count": 34,
  "fact_count": 0,
  "attempt": 2,
  "recovered_after_retry": false
}
```

The label is built with `serde_json::json!` (no raw payload substring, no ANSI
bytes, no unescaped newline can leak into a metrics line) and, like every distill
metric write, is a **no-op under `cfg!(test)`** so unit tests never append to the
operator's real `~/.simard/metrics/metrics.jsonl`.

**Implementation touchpoints (single escalation site).** The shape is computed
once, at the distill escalation site where the `SimardError` (`e`) and its
`DistillFailureClass` (`classify_distill_error(&e)`) are both already in hand, and
threaded from there into two existing sinks — no new call path and no re-parse:

- `build_distill_success_context` / `record_distill_success_metric` take a new
  `shape: Option<ParseFailureShape>` argument that adds the `parse_failure_shape`
  field (via `as_str`, `null` when `None`). Their existing `class` parameter is
  unchanged, so the denominator gate (`parse_attempted`) and shared
  `distill_success_rate`/`distill_parse_success_rate` context are untouched.
- `raw_capture::CaptureMeta` gains a matching `parse_failure_shape` field and the
  capture-header writer emits one additional `# parse_failure_shape:` line beside
  the existing `# failure_class:` line.

Read the split back with the same query you already use for the rate:

```bash
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
  | grep '"parse_success":false' \
  | grep -o '"parse_failure_shape":"[a-z-]*"' \
  | sort | uniq -c | sort -rn
```

A result dominated by `missing-file` / `empty-document` is the gate signal that
parser work will not help; a material `unparseable-object` share is the signal
that a `strip_json_<defect>` transform is worth adding.

When [raw-capture](./distill-raw-capture-on-parse-failure.md) is enabled, each
harvested sample header is tagged with the same `parse_failure_shape`, so a
captured payload correlates 1:1 with its `metrics.jsonl` event and its shape.

## Configuration

Shape classification and the lenient transforms are **always on** and require no
configuration — they add measurement and strict-parse-first recovery with no
behavior change on the clean path. The only related knob is the existing,
default-off raw-capture toggle used to *harvest* the `unparseable-object` samples
that justify a new transform:

| Variable | Default | Purpose |
| --- | --- | --- |
| `SIMARD_DISTILL_RAW_CAPTURE` | `0` (off) | Enable raw-byte capture of surviving `ParseFailure`s so an `unparseable-object` sample can be turned into a regression fixture. See [Distill raw-capture on parse failure](./distill-raw-capture-on-parse-failure.md). |

There is intentionally no toggle to disable strict-parse-first or the transform
chain: the clean path is byte-identical and the transforms are provable no-ops on
valid JSON, so there is nothing to turn off.

## Transform authoring contract

To add a new `strip_json_<defect>` (only after S1 evidence shows the defect):

1. **Harvest a real sample** with `SIMARD_DISTILL_RAW_CAPTURE=1` and confirm its
   shape is `unparseable-object`. Never write a transform for a hypothetical
   defect.
2. **Write the transform** as `fn(&str) -> Cow<'_, str>` in
   `recipe_output::extract`, string-literal aware, single-pass, no regex, no
   input-depth recursion.
3. **Prove it with four paired tests:**
   - valid JSON in ⇒ `Cow::Borrowed` (no-op proof),
   - the defect inside a string literal ⇒ **byte-identical value** (string-aware
     proof),
   - the targeted defect ⇒ recovers to a parseable object,
   - a non-recoverable object ⇒ still `None`/`Err` (precision proof).
4. **Chain it** into `parse_facts_envelope_lenient` after the existing views, and
   **re-export** it from `recipe_output::mod`.
5. **Add an integration test** through the harvest path proving the grounding gate
   still rejects forged/source-less facts.

Because the chokepoint is shared, a vetted transform benefits the brain and OODA
recipe parse paths too, not just distill.

## Invariants preserved

- **Strict-parse-first.** `serde_json::from_str` is always tried before any
  transform, so well-formed output is parsed byte-for-byte as before.
- **Provable no-op on valid JSON.** Every transform returns `Cow::Borrowed`
  unchanged when its defect is absent — zero allocation, zero behavior change on
  the clean path.
- **Closed target type.** Parsing always targets `RecipeEnvelope`; there is no
  `serde_json::Value`/flatten widening that could accept arbitrary shapes.
- **Grounding gate authoritative.** The grounded-capable > non-empty > empty
  preference tiers still choose which balanced object wins; leniency only changes
  *whether* a candidate parses, never *which* candidate is trusted.
- **`Err` never marks episodes.** A surviving `UnparseableObject` still returns
  `Err`; no markers are set and the batch retries (non-fatal).
- **Non-zero exit never reaches leniency.** A non-zero recipe exit or a runner
  `Err` is classified as a `spawn` / `copilot-terminal` / `recipe-reported`
  failure *before* the facts document is ever parsed, so no transform can turn a
  terminal failure into a parsed success. (The facts `RecipeEnvelope` carries only
  `facts`/`procedures` — there is no `success` field in this path for leniency to
  override.)
- **Metric stability.** No new metric name, no denominator change; the
  `parse_failure_shape` field is additive and `null` outside `ParseFailure`.
- **Test isolation.** Metric writes remain `cfg!(test)`-gated (and the raw-capture
  tag remains behind the default-off toggle), so tests never touch real
  `metrics.jsonl`.

## Security model

- **Adversary-shaped input.** The parser consumes semi-trusted LLM/agent output;
  the JSON under parse is attacker-influenced. No new auth, network, process-spawn,
  or env-driven-exec surface is introduced.
- **Bounded, single-pass, no regex.** Every transform is O(n) with no input-depth
  recursion, so there is no ReDoS or stack-overflow vector on adversarial input.
- **No in-string corruption.** The string-aware scan guarantees a defect byte
  inside a legitimate JSON string literal is preserved byte-for-byte (tested).
- **Labels only in metrics.** `parse_failure_shape` is a fixed enum label; the raw
  payload never reaches `metrics.jsonl` (see
  [metrics hygiene](./distill-raw-capture-on-parse-failure.md#metrics-hygiene)).
  Error strings remain 200-char truncated.
- **Precision over recall.** A transform that would accept forged or malformed
  facts is rejected by the authoring contract; `clippy -D warnings` blocks any
  `unwrap`/panic on the parse path, and a non-recoverable object must still `Err`.

## Examples

### Read the measured A/B/C split

```bash
# What is the *current* parse-failure shape distribution?
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
  | grep '"parse_success":false' \
  | grep -o '"parse_failure_shape":"[a-z-]*"' \
  | sort | uniq -c | sort -rn
#   12 "parse_failure_shape":"empty-document"
#    3 "parse_failure_shape":"unparseable-object"
#    1 "parse_failure_shape":"missing-file"
```

Here `empty-document` dominates: the residual is source-side, and prompt/retry
tuning — not a new transform — is the correct next step.

### Recover a fence-wrapped object

The agent wraps its answer in a Markdown code fence (input, left) and the
transform yields the bare object (output, right):

~~~text
input  (agent wrote):          output (recovered):

```json                        {"facts":[ … ],"procedures":[ … ]}
{"facts":[ … ],
 "procedures":[ … ]}
```
~~~

`strip_json_code_fences` returns an `Owned` view with the fence removed;
`parse_facts_envelope_lenient` re-parses it strictly into a `RecipeEnvelope`. A
fence-free, already-valid object returns `Cow::Borrowed` and is untouched.

### Confirm a transform is a no-op on valid input

```rust
use std::borrow::Cow;
use simard::recipe_output::strip_json_code_fences;

let clean = r#"{"facts":[],"procedures":[]}"#;
assert!(matches!(strip_json_code_fences(clean), Cow::Borrowed(_)));
```

## When shape classification does *not* apply

- The pass **succeeded** (including a success recovered by a transform or by the
  in-cycle retry): `parse_failure_shape` is `null`.
- The failure class is **not** `ParseFailure` — a `spawn-failure`,
  `copilot-terminal-failure`, `recipe-reported-failure`, or `serialize-failure`
  never reached output parsing, so it has no shape (`null`) and emits no
  `distill_parse_success_rate` event at all.
- The pass was below `DISTILL_MIN_EPISODES` and skipped the recipe entirely (no
  metric event).

This keeps the shape field meaningful exactly where it exists to be: inside the
`ParseFailure` bucket, telling you whether the parser can help.

## Related

- [Distill recipe output capture](./distill-recipe-output-capture.md) — the
  `ParseFailure` class and facts-file channel this page sub-classifies.
- [Distill raw-capture on parse failure](./distill-raw-capture-on-parse-failure.md)
  — the diagnostic that harvests the `unparseable-object` samples justifying a
  transform.
- [Text-parsing wire formats](./text-parsing-wire-formats.md) — the shared
  `recipe_output::extract` primitives (`balanced_objects`,
  `strip_json_trailing_commas`, …).
- [Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md)
  — the shared chokepoint the transforms live in.
- [Episode distillation](../architecture/episode-distillation.md) — the
  surrounding pipeline.
- [Capture and diagnose a failing distill sample](../howto/capture-and-diagnose-a-failing-distill-sample.md)
  — the step-by-step harvesting how-to.
