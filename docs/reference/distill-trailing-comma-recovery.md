---
title: Distill trailing-comma recovery
description: Reference for the string-aware trailing-comma recovery pass that lets the distillation parser accept an otherwise well-formed facts document whose only defect is a JSON-illegal trailing comma — the shared recipe_output::strip_json_trailing_commas helper, its Cow-returning total contract and string-literal safety invariants, the strict-first/repair-on-Err-only recovery tier wired into scan_cleaned_for_facts, the ParseRecovery discriminator and its append-only parse_recovery key in metrics.jsonl, the distinct "parsed OK, zero facts after category filter" warn, and the never-a-hollow-Ok guarantee that keeps genuinely malformed input deferring for a safe retry.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: design — not yet implemented
related:
  - ./distill-recipe-output-capture.md
  - ./distill-raw-capture-on-parse-failure.md
  - ./text-parsing-wire-formats.md
  - ./telemetry-metrics.md
  - ../howto/observe-distill-trailing-comma-recovery.md
  - ../howto/capture-and-diagnose-a-failing-distill-sample.md
  - ../architecture/episode-distillation.md
  - ../../src/recipe_output/extract.rs
  - ../../src/memory_consolidation/distillation.rs
---

# Distill trailing-comma recovery

> **Status: specified for issue #2669 — lands with the implementing PR.** This
> reference is the spec-first contract for the change; the code and these docs
> merge together, so the `main` links below resolve once that PR lands. The
> string-aware trailing-comma recovery pass will live in
> [`src/recipe_output/extract.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/extract.rs)
> as the shared `strip_json_trailing_commas` helper, wired into the
> facts-document parser
> [`scan_cleaned_for_facts`](https://github.com/rysweet/Simard/blob/main/src/memory_consolidation/distillation.rs).
> It closes the recurring `overseer-obs:anomaly:distill parse-fail rate 100%`
> signature: a distill agent that emitted an otherwise valid
> `{ "facts": [...] }` object with a single **trailing comma** kept the braces
> balanced (so it passed `recipe_output::balanced_objects`) but was **rejected
> by strict `serde_json`**, so every span failed, the batch deferred forever,
> and the parse-fail rate pinned at 100%.

The distillation pass turns batches of episodes into semantic facts by shelling
out to `recipe-runner-rs` and parsing the distill agent's
`{ "facts": [...], "procedures": [...] }` JSON from a dedicated facts file. The
parser (`parse_facts_document` → `scan_cleaned_for_facts`) uses **strict**
`serde_json::from_str::<RecipeEnvelope>` as the sole arbiter of validity. Strict
serde correctly rejects a JSON trailing comma (`[…, ]` or `{…, }`), but LLM
agents emit them routinely. Because a trailing comma does not unbalance braces,
every upstream chokepoint (ANSI stripping, launch-preamble stripping, balanced
span extraction) passed the candidate through unchanged — only the final strict
parse failed, and it failed **every time** for that document.

Trailing-comma recovery adds a **second, narrower parse attempt** that runs
**only after** the strict parse of a candidate returns `Err`. It removes
JSON-illegal trailing commas — and nothing else — then re-runs the same strict
`serde_json`. If the repaired view parses, the document is recovered; if it
still fails, the pass defers exactly as before (retry-safe, never a hollow
`Ok`). The repair is **string-aware**: commas, braces, and brackets inside JSON
string literals are never touched, so fact content is preserved byte-for-byte.

For the envelope parse contract this recovery instruments, see
[Distill recipe output capture](./distill-recipe-output-capture.md). For the
shared byte-scanning helpers this pass joins, see
[Text-parsing wire formats](./text-parsing-wire-formats.md).

## Contents

- [Why](#why)
- [How recovery works](#how-recovery-works)
- [Public API](#public-api)
  - [`strip_json_trailing_commas` (the repair helper)](#strip_json_trailing_commas-the-repair-helper)
  - [`scan_cleaned_for_facts` (the recovery tier)](#scan_cleaned_for_facts-the-recovery-tier)
  - [`ParseRecovery` (the outcome discriminator)](#parserecovery-the-outcome-discriminator)
- [Metrics data contract](#metrics-data-contract)
- [The zero-facts-after-filter warning](#the-zero-facts-after-filter-warning)
- [Guarantees](#guarantees)
- [Security model](#security-model)
- [Examples](#examples)
- [When recovery does *not* fire](#when-recovery-does-not-fire)
- [Related](#related)

## Why

Post-#2622/#2619, every recipe-backed phase — decide, orient, and the
**distill** pass — routes through the shared, hardened extractor in
[`src/recipe_output/extract.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/extract.rs),
which strips ANSI colour codes, timestamped log lines, the runner banner, and
the Copilot CLI launch preamble, then hands strict `serde_json` a set of
brace-balanced spans. That eliminated the *noise* class of parse failure.

A **trailing comma** is a different failure. It is not noise the extractor can
strip and it does not unbalance the object, so it survives every existing
chokepoint and is rejected only at the final strict parse. The distill agent
emitting one trailing comma per document therefore produced a **100%** parse
failure for that document, indefinitely — the observed signature. No lenient
recovery existed anywhere in the parse path, and widening `serde_json` itself
(or the `RecipeEnvelope` field coercion) would have relaxed validity for *all*
inputs. Trailing-comma recovery is the minimal, targeted alternative: a
byte-level repair that is attempted **only** on a strict failure and removes
**only** trailing commas.

## How recovery works

```
distill agent → facts file ──► parse_facts_document
                                  └─► scan_cleaned_for_facts
                                        │  for the fast path AND each balanced span:
                                        │
                                        │  1. serde_json::from_str::<RecipeEnvelope>(span)   ← strict, FIRST
                                        │         │ Ok  → StrictOk
                                        │         │ Err → step 2
                                        │  2. serde_json::from_str::<RecipeEnvelope>(
                                        │         strip_json_trailing_commas(span).as_ref())  ← repair, ON Err ONLY
                                        │         │ Ok  → Recovered
                                        │         │ Err → candidate skipped
                                        ▼
                     RecipeEnvelope::into_output
                        │  (parsed, but 0 facts after category filter → distinct warn → ZeroFacts)
                        ▼
                     build_distill_success_context(… parse_recovery)
                        │  append-only "parse_recovery" key
                        ▼
                     record_metric("distill_success_rate" / "distill_parse_success_rate", …)
                        → ~/.simard/metrics/metrics.jsonl

   No candidate parsed under EITHER view
                        → None → parse_facts_document Err → caller defers (retry-safe)  → Deferred
```

The three existing preference tiers (grounded-capable → non-empty → empty) and
the END-first span iteration order are **unchanged**. Recovery only changes
whether a candidate *parses at all*; it never reorders preference and never
changes what a field means.

## Public API

### `strip_json_trailing_commas` (the repair helper)

**File:** `src/recipe_output/extract.rs` — joins `balanced_objects`,
`scan_balanced`, `strip_ansi`, and `strip_recipe_noise`.

```rust
/// Remove JSON-illegal trailing commas — a `,` whose next non-whitespace byte
/// is `}` or `]` — that make strict `serde_json` reject an otherwise
/// well-formed value.
///
/// STRING-AWARE: commas, braces, and brackets inside JSON string literals are
/// never touched (mirrors `scan_balanced`'s `in_string`/`escaped` machine).
///
/// Returns `Cow::Borrowed(s)` unchanged when no trailing comma is present, so
/// the common (clean) parse path allocates nothing. Total and pure: never
/// panics, never returns `Result` — strict `serde_json` downstream remains the
/// sole arbiter of validity.
pub fn strip_json_trailing_commas(s: &str) -> std::borrow::Cow<'_, str>;
```

| Aspect | Guarantee |
| --- | --- |
| Input | Any `&str`. Intended: a brace-balanced, noise-stripped span, but the fn is **total** over arbitrary input. |
| Output | `Cow::Borrowed` iff byte-identical (no offending comma); else `Cow::Owned` with only trailing commas removed. |
| **I1 — string safety** | Bytes inside `"…"` are preserved exactly, honoring `\"` and `\\` escapes. `{"c":"a,}"}` is returned unchanged. |
| **I2 — minimality** | Removes **only** a `,` whose next non-whitespace (` \t\r\n`) byte is `}` or `]`. No other byte is altered. A trailing comma before EOF (no closer) is left as-is — still invalid, still `Err` downstream. |
| **I3 — purity** | No panic, no allocation on the clean path, no `Result`. |
| **I4 — idempotence** | `f(f(x)) == f(x)`. |
| Non-goals | No json5, comments, quote-fixing, or single→double quotes. Trailing commas only. |

**Errors:** none by construction. A still-malformed stripped result simply fails
the subsequent strict `serde_json::from_str`, preserving the retry-safe
deferral.

**Implementation shape:** a single forward pass over the bytes mirroring
`scan_balanced`'s `in_string`/`escaped` state machine. It branches only on ASCII
bytes (`,` `}` `]` `"` `\`, and ASCII whitespace), so byte-indexed decisions are
UTF-8-safe and multi-byte / emoji content is preserved. Iterative `O(n)`, no
recursion.

**Why it lives in `recipe_output`, not `distillation`:** the same
strict-serde-vs-noisy-agent-JSON problem exists for every recipe parse path
(brain decide/orient, merge-judge verdicts). Co-locating the helper with
`balanced_objects`/`scan_balanced` keeps the string-aware invariant in one
audited place and makes it reusable, with **zero coupling** to distillation
types (the helper is generic over `&str`).

### `scan_cleaned_for_facts` (the recovery tier)

**File:** `src/memory_consolidation/distillation.rs`.

```rust
// Signature UNCHANGED
fn scan_cleaned_for_facts(trimmed: &str) -> Option<DistillOutput>;
```

For the fast path and **each** balanced span, the parse order is:

1. `serde_json::from_str::<RecipeEnvelope>(candidate)` — strict, first.
2. **On `Err` only:**
   `serde_json::from_str::<RecipeEnvelope>(strip_json_trailing_commas(candidate).as_ref())`.

| Guarantee | Detail |
| --- | --- |
| Never a hollow `Ok` | Input that parses under **neither** view → candidate skipped → `None` → `parse_facts_document` returns `Err` → caller defers (retry-safe). |
| Tier ordering intact | A `Recovered` grounded-capable span still beats a `StrictOk` empty span. Recovery does not reorder preference. |
| No new leniency | Only trailing-comma removal; field-level coercion (`de_lenient_string`) and the `RecipeEnvelope` shape are untouched. |

### `ParseRecovery` (the outcome discriminator)

A four-variant enum records how a pass's facts document parsed, so a repaired
parse is observably different from a clean one. It is **orthogonal** to the
existing `recovered_after_retry` flag (see
[Metrics data contract](#metrics-data-contract)).

```rust
/// How the *facts document* for one pass was parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseRecovery {
    /// Strict `from_str` succeeded on the raw candidate — no repair.
    StrictOk,
    /// Strict failed on the raw candidate but succeeded after
    /// `strip_json_trailing_commas`. The #2669 fix path.
    Recovered,
    /// No candidate parsed under either view → pass deferred (`Err`).
    Deferred,
    /// A candidate parsed but 0 facts survived the category filter.
    ZeroFacts,
}

impl ParseRecovery {
    /// Stable label for the metric context. NEVER rename these strings —
    /// downstream `metrics.jsonl` readers key on them.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StrictOk  => "strict-ok",
            Self::Recovered => "recovered",
            Self::Deferred  => "deferred",
            Self::ZeroFacts => "zero-facts",
        }
    }
}
```

The four `as_str` strings are a **frozen vocabulary** — treat them as a public
enum for `metrics.jsonl` consumers. Adding a *new* variant later is additive and
allowed; renaming an existing one is a breaking change and forbidden without a
coordinated reader update.

## Metrics data contract

`parse_recovery` is added as an **append-only** key in the `context` object of
the distill success metrics. The producer signature is unchanged; only the
context builder gains one parameter.

```rust
// src/self_metrics/mod.rs — UNCHANGED
pub fn record_metric(metric_name: &str, value: f64, context: &str)
    -> Result<(), Box<dyn std::error::Error>>;

// src/memory_consolidation/distillation.rs — one appended parameter
fn build_distill_success_context(
    success: bool,
    class: Option<DistillFailureClass>,
    input_count: u32,
    fact_count: u32,
    attempt: u32,
    recovered_after_retry: bool,
    parse_recovery: ParseRecovery,   // NEW
) -> String;
```

The metric envelope is `MetricEntry` (`{ timestamp, metric_name, value,
context }`), one JSON object per line in `~/.simard/metrics/metrics.jsonl`. The
classification counters live inside the stringified `context` field.

### Context JSON schema (`distill_success_rate` / `distill_parse_success_rate`)

| Key | Type | Status | Notes |
| --- | --- | --- | --- |
| `outcome` | `"success"` \| `"failure"` | existing | unchanged |
| `recipe_exited_ok` | bool | existing | unchanged |
| `parse_attempted` | bool | existing | unchanged |
| `parse_success` | bool | existing | unchanged |
| `failure_class` | string \| null | existing | `DistillFailureClass::as_str` |
| `input_count` | u32 | existing | unchanged |
| `fact_count` | u32 | existing | unchanged |
| `attempt` | u32 | existing | 1-based runner invocations |
| `recovered_after_retry` | bool | existing | recovered via a whole-runner **re-invocation** (#2468) |
| **`parse_recovery`** | **string** | **NEW** | one of `strict-ok` \| `recovered` \| `deferred` \| `zero-facts` |

**`parse_recovery` vs `recovered_after_retry` — do not conflate:**

- `recovered_after_retry = true` → success followed a **new whole-runner
  invocation** (`attempt > 1`). Axis: *re-run the agent*.
- `parse_recovery = recovered` → the **same** facts document parsed only after
  trailing-comma stripping, **within one invocation**. Axis: *repair the bytes*.

Both can be true independently.

### Versioning

- **Append-only, additive.** No existing key is renamed, removed, or retyped.
  Readers that ignore `parse_recovery` are unaffected — the means over
  `distill_success_rate` / `distill_parse_success_rate` are numerically
  **identical** to before this change (Decision D-1: reuse the existing channel,
  no new metric name, unchanged denominators).
- **No `schema_version` field is introduced** — `metrics.jsonl` has never
  carried one and mean-based consumers are forward-compatible with unknown keys.
- **Best-effort & test-silent.** Write errors are logged, not propagated;
  `record_*` is a no-op under `cfg!(test)` (unchanged).

A high `recovered` share is the **detection signal** for a recurring agent bug
hiding behind auto-repair — it is queryable, not silently absorbed. See the
[how-to](../howto/observe-distill-trailing-comma-recovery.md) for the query.

## The zero-facts-after-filter warning

A valid parse that yields **0 facts after the category filter**
(`pr-pattern` | `bug-pattern` | `lesson-learned`, via `canonical_distill_concept`)
must never be conflated with a parse failure. `RecipeEnvelope::into_output`
emits a **distinct** structured warning when — and only when — the envelope
parsed but every fact was dropped by the filter:

```rust
tracing::warn!(
    target: "simard::distill",
    input_facts = <pre_filter_len>,
    kept_facts  = 0,
    "distill: envelope parsed but all facts were dropped by the category filter \
     (pr-pattern|bug-pattern|lesson-learned); not a parse failure",
);
```

| Guarantee | Detail |
| --- | --- |
| Fires only on real loss | Warn **iff** `pre_filter_len > 0 && kept == 0`. A legitimately empty `{"facts":[]}` envelope (`pre_filter_len == 0`) does **not** warn. |
| Snapshot the count before the move | `into_output` **moves** `facts` inline into the split envelopes, so the implementation must capture `pre_filter_len = facts.len()` **before** that move and compare it against the kept count returned by `into_facts`. Reading `facts.len()` after the move is a use-after-move bug. |
| Count-only | Logs only the two integers and the fixed message — no fact `concept`/`content`/`source_episode_id`, no document bytes. |
| Side-channel only | Logging only; return type and control flow are unchanged. |
| Namespace | `target: "simard::distill"`, matching existing distill warns for operator filtering. |

This surfaces as `parse_recovery = zero-facts` in the metric context, so
"parsed OK, all facts filtered out" is distinguishable from both a clean parse
and a parse failure.

## Guarantees

The recovery path preserves the distillation pipeline's safety invariants:

- **Never a hollow `Ok`.** Recovery only turns a *repairable* `Err` into `Ok`.
  Genuinely malformed input stays `Err` all the way to `parse_facts_document`,
  so the batch defers and retries — it is never silently reported as
  parsed-empty.
- **Strict-first ordering.** Strict `serde_json` runs before any repair; the
  repair parse runs only in the `Err` arm. `serde_json` remains the single
  arbiter of validity.
- **String-interior bytes untouched.** Commas/braces/brackets inside string
  literals are preserved, honoring `\"` and `\\`.
- **Content gates remain mandatory.** After repair+parse, every existing gate
  still runs: grounded-capable tier (non-empty `source_episode_id`), reliability
  quarantine (`assess_fact_reliability`), empty-`concept` drop, and
  closed-set category canonicalization (`canonical_distill_concept` /
  `KNOWN_DISTILL_CONCEPTS`). The repair adds no shortcut that trusts content.
- **Observability over silent repair.** `parse_recovery = recovered` makes an
  auto-repaired parse distinguishable from a clean one.
- **Append-only metrics.** Existing metric consumers and denominators are
  unaffected.

## Security model

The facts document is **fully untrusted** LLM output. The recovery pass widens
exactly one trust boundary — the untrusted-input parse — and is hardened so it
cannot become a second, weaker parser.

- **No new attack surface.** No new authn/authz surface, network listener, or
  IPC endpoint. The repair pass and metric are pure and local; the
  `recipe-runner-rs` subprocess keeps the parent's existing privilege.
- **Totality / no panic.** `strip_json_trailing_commas` is total over any
  `&str` — unterminated string, lone trailing backslash at EOF, no closer,
  all-commas, empty, whitespace-only — and never panics (no indexing panic, no
  slice on a non-char boundary).
- **UTF-8 boundary safety.** Scanning is on bytes but branches only on ASCII;
  any owned rebuild reuses original byte spans, so multi-byte / emoji content
  (`{"c":"🎉,"}`) is preserved exactly.
- **String-literal awareness.** Commas, braces, and brackets inside `"…"` are
  never altered, honoring `\"` and `\\`. Adversarial cases (`{"c":"a,}"}`,
  `{"c":"x,]"}`, escaped-quote-then-comma) are covered by tests.
- **Minimality.** Only a `,` whose next non-whitespace byte is `}` or `]` is
  removed. This bounds the repair's power so it cannot manufacture a
  valid-looking object from genuinely malformed or hostile bytes.
- **Never a hollow `Ok`** (anti-injection invariant). Input that parses under
  neither view defers; no unvalidated data enters `DistillOutput`.
- **Log hygiene.** The metric context and the zero-facts warn carry **counts and
  frozen-enum labels only** — never fact content, concepts, source-episode IDs,
  or document excerpts. `ParseRecovery::as_str` is four fixed ASCII strings.
- **Bounded work.** The repair is a single iterative `O(n)` pass (no recursion);
  `serde_json`'s built-in 128-deep nesting bound is left intact, so deeply
  nested adversarial JSON is rejected, not accepted. A single strict→repair→strict
  attempt (no repair loop) bounds the work per candidate.
- **Private capture path preserved.** The `0700` per-invocation tempdir and
  drop-time cleanup for the facts file are unchanged.

The net effect is that the fix **narrows** the recurring silent-drop failure
**without widening** the attack surface, and the one net-new artifact
(`parse_recovery = recovered`) doubles as the detection control for repair abuse
or a regressing agent.

## Examples

### A recovered document

Agent output (note the trailing comma after the last fact object):

```json
{
  "facts": [
    { "concept": "bug-pattern", "content": "retry storms on 429", "source_episode_id": "ep-8842" },
  ]
}
```

- Strict `serde_json::from_str::<RecipeEnvelope>` → `Err` (trailing comma).
- `strip_json_trailing_commas` drops the `,` before `]` → the view now parses.
- Result: the fact is extracted; the metric records
  `parse_recovery = "recovered"`.

### A comma inside a string is left alone

```json
{ "facts": [ { "concept": "lesson-learned", "content": "log line: a, b, }", "source_episode_id": "ep-1" } ] }
```

The `,` characters inside `"log line: a, b, }"` are **not** trailing commas —
their next non-whitespace byte is inside the string literal, not a real `}`/`]`
— so the content is preserved byte-for-byte.

### Genuinely malformed input still defers

```json
{ "facts": [ { "concept": "bug-pattern"
```

Neither the strict view nor the stripped view parses (the object is truncated),
so `scan_cleaned_for_facts` returns `None`, `parse_facts_document` returns `Err`,
and the batch **defers for a safe retry**. The metric records
`parse_recovery = "deferred"` — never a hollow `Ok`.

### Confirm recovery from the metrics stream

```bash
# Passes that were rescued by trailing-comma repair. `context` is a *stringified*
# JSON embedded in each record (self_metrics/mod.rs serialises the whole entry
# with serde_json::to_string), so the inner quotes are escaped on the raw line
# (\"parse_recovery\":\"recovered\"). Select on the *decoded* value with jq's
# `fromjson`; a raw `grep '"parse_recovery":"recovered"'` matches zero lines.
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
  | jq -c 'select((.context | fromjson).parse_recovery == "recovered")' | tail -n 20
```

See [Observe distill trailing-comma recovery](../howto/observe-distill-trailing-comma-recovery.md)
for the full verification runbook, including confirming the overseer anomaly
self-heals once the parse-fail rate drops.

## When recovery does *not* fire

Recovery is intentionally narrow. The stripped-view parse is **not** attempted,
or has no effect, when:

- The strict parse already **succeeded** — the raw candidate is used verbatim
  (`parse_recovery = strict-ok`); the clean path allocates nothing.
- The defect is anything **other than** a trailing comma — missing quotes,
  truncated objects, comments, single quotes, or json5 syntax. These stay `Err`
  and the pass defers (`parse_recovery = deferred`). Recovery does **not** widen
  acceptance beyond trailing-comma removal.
- The candidate has a comma that is **not** JSON-illegal (a separator between
  elements, or a comma inside a string literal). Nothing is removed.
- The pass never reached parsing (a `spawn-failure`,
  `copilot-terminal-failure`, or `recipe-reported-failure`) — there is no facts
  document to repair.

## Related

- [Distill recipe output capture](./distill-recipe-output-capture.md) — the
  `RecipeEnvelope` / three-tier parser contract this recovery instruments.
- [Distill raw-capture on parse failure](./distill-raw-capture-on-parse-failure.md)
  — the env-gated diagnostic for harvesting a still-failing sample.
- [Text-parsing wire formats](./text-parsing-wire-formats.md) — the shared
  `recipe_output` byte-scanning helpers this pass joins.
- [Telemetry metrics](./telemetry-metrics.md) — the `metrics.jsonl` envelope and
  reader surface the `parse_recovery` key extends.
- [Observe distill trailing-comma recovery](../howto/observe-distill-trailing-comma-recovery.md)
  — the step-by-step verification how-to.
- [Episode distillation](../architecture/episode-distillation.md) — the
  surrounding pipeline.
