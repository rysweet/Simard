---
title: Distillation parse-failure-rate hardening
description: The consolidated reference for the distillation parse-resilience contract that drove the ~85% parse-failure rate back toward zero — the banner-immune facts-file channel, field-tolerant deserialization, the single-trailing-comma repair, strict rejection of genuinely malformed output, the first-class distill_parse_success_rate metric, and the zero_facts_reason disposition (none / true_empty / all_quarantined) that distinguishes "nothing worth distilling" from "the reliability gate blocked everything" without a cross-metric join.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./distill-recipe-output-capture.md
  - ./distill-raw-capture-on-parse-failure.md
  - ./automatic-distillation-scheduler.md
  - ./telemetry-metrics.md
  - ./text-parsing-wire-formats.md
  - ../architecture/episode-distillation.md
  - ../howto/capture-and-diagnose-a-failing-distill-sample.md
  - ../../src/memory_consolidation/distillation.rs
  - ../../src/recipe_output/extract.rs
  - ../../src/self_metrics/mod.rs
---

# Distillation parse-failure-rate hardening

> **Status — implemented.** This page is the authoritative, consolidated
> reference for the distillation *parse-resilience* contract: the set of fixes
> that took the distill pass's `parse-failure` rate from a live spike of
> **~85–100%** back toward **~0% for recoverable output**, and the metrics that
> let an operator *measure* that rate before and after. Present tense below
> describes shipped behavior — including the `zero_facts_reason` disposition
> (and the `candidate_facts` context field that feeds it), which landed with the
> R1 change on this branch and is now emitted at HEAD. Locations:
> parser + metrics `src/memory_consolidation/distillation.rs`;
> trailing-comma repair `src/recipe_output/extract.rs`;
> metric envelope `src/self_metrics/mod.rs`;
> tests `src/memory_consolidation/distillation_tests.rs`, the zero-facts unit
> tests in `distillation.rs`, and the hermetic
> `issue_2622_file_channel_tests` in `distillation.rs`.

The episode-distillation pass turns batches of episodic memory into semantic
**facts** and reusable **procedures** by shelling out to `recipe-runner-rs` and
reading the distill agent's `{ "facts": [...], "procedures": [...] }` JSON. When
that JSON fails to parse, the pass classifies as a
[`ParseFailure`](./distill-recipe-output-capture.md): it sets no markers, the
batch retries next cycle, and **zero facts** are learned. A sustained
parse-failure rate therefore silently starves memory of new facts while burning
an LLM call every cycle.

This page describes the finished state of the hardening as one contract:

1. **What is now recovered** — the recovery tiers that turn once-fatal cosmetic
   defects (banner contamination, one bad field, a single trailing comma) into
   successful parses.
2. **What is still (correctly) rejected** — the strict-`Err` boundary that keeps
   precision: empty output, genuinely malformed JSON, and non-zero recipe exits
   stay explicit failures, never hollow successes.
3. **How the rate is measured** — the `distill_parse_success_rate` metric and its
   context payload, including the `zero_facts_reason` disposition that separates
   a *true-empty* success from an *all-quarantined* success.

For the underlying file-channel capture mechanism this contract builds on, see
[Distill recipe output capture](./distill-recipe-output-capture.md). For the
env-gated diagnostic that harvests a *residual* live failure into a regression
fixture, see
[Distill raw-capture on parse failure](./distill-raw-capture-on-parse-failure.md).

## Contents

- [Root cause](#root-cause)
- [Recovery tiers — what now parses](#recovery-tiers-what-now-parses)
- [Strict rejection — what still fails](#strict-rejection-what-still-fails)
- [Zero-facts disposition](#zero-facts-disposition)
- [Metrics](#metrics)
- [Configuration](#configuration)
- [Public API](#public-api)
- [Examples](#examples)
- [Tutorial — measure the before/after rate](#tutorial-measure-the-beforeafter-rate)
- [Testing](#testing)
- [Non-goals](#non-goals)
- [Related](#related)

## Root cause

The ~85–100% `parse-failure` spike had three distinct contributing shapes. All
three are now closed, each by a *structural* fix rather than an ever-more-lenient
string scan:

| # | Failing shape | Root cause | Structural fix |
| --- | --- | --- | --- |
| 1 | **Banner contamination** | The distill result was scraped from `recipe-runner-rs` **stdout**, which carries the Copilot CLI launcher banner (`… launching copilot binary=…`, `ℹ NODE_OPTIONS=… (saved preference)`) *around* the agent's answer. A stdout scan for `{ "facts": [...] }` matched the banner, not the answer. | The agent now **writes** its JSON to a dedicated, per-invocation facts file (`-c facts_output_path=…`); stdout is never read as the result. A banner on stdout can no longer reach the parser (issues #2622 / #2619). |
| 2 | **One malformed field sinks the batch** | A single `facts[]` entry with a missing / `null` / bare-scalar field (or an explicit `"procedures": null`) made strict `serde` reject the **entire** envelope, dropping every well-formed sibling. | Field-tolerant deserialization: an **id-like** field (`source_episode_id`, procedure `name`) coerces an off-spec scalar to the empty string (`de_lenient_string` — an episode id is legitimately numeric); a **text** field (`concept`, `content`) requires a real JSON string and otherwise collapses to empty (`de_string_only`); and an **optional array** (`procedures`, `steps`, `source_episode_ids`) tolerates an explicit `null` as `[]` (`de_null_tolerant_vec`). The reliability gate then quarantines the one bad fact instead of dropping the whole batch (issues #2506, #2431). |
| 3 | **One trailing comma drops the batch** | A single `,` before a closing `}`/`]` — the most common real-world LLM-JSON defect — is *never valid JSON*, so strict `serde` rejected the whole facts object. | A last-resort, string-aware `strip_json_trailing_commas` repair retries the parse on a comma-stripped view; it is a provable no-op on already-valid JSON (issue #2658). |

**No further parser leniency is warranted.** The parser is strict on the clean
facts-file channel and lenient *only* on provably-safe cosmetic defects, so
recovery never widens to accept genuinely broken JSON (precision is preserved).

## Recovery tiers — what now parses

`parse_facts_document` applies these tiers in order. Each tier is *additive*: a
document that a stricter tier already parsed never reaches a looser one.

| Tier | Input shape | Mechanism | Outcome |
| --- | --- | --- | --- |
| 0 | Exact `{ "facts": [...], "procedures": [...] }` | strict `serde_json` | parse |
| 1 | Envelope wrapped in a Markdown code fence or a little leading/trailing prose in the file | `scan_cleaned_for_facts` → `balanced_objects` picks the **last** balanced `{…}` carrying a grounded fact | parse |
| 2 | An off-spec `facts[]` field (missing / `null` / scalar), or an explicit `null` for an optional array (`procedures` / `steps` / `source_episode_ids`) | `de_lenient_string` coerces an id-like scalar; `de_string_only` collapses a non-string `concept`/`content` to `""`; `de_null_tolerant_vec` maps a `null` array to `[]` | parse; the bad fact/procedure is later quarantined or dropped, siblings survive |
| 3 | A single **trailing comma** before a `}`/`]`, anywhere outside a JSON string literal | `strip_json_trailing_commas` retry (only after strict parse fails) | parse |

Key guarantees of the tiers:

- **Last-object preference.** `balanced_objects` returns candidates scanned from
  the **end**, so a leading "thinking"/banner object (or a stray `{` in the
  agent's prose, e.g. `fn f() {`) cannot shadow the real answer that follows it.
- **String-aware comma repair.** `strip_json_trailing_commas` tracks quote /
  escape state, so a comma *inside* a fact's `content` string is preserved
  byte-for-byte. Only a comma immediately preceding `}`/`]` (ignoring
  intervening ASCII whitespace) is removed, and every removed byte is ASCII, so
  the result is always valid UTF-8.
- **No-op on valid JSON.** The repair returns the input borrowed and unchanged
  whenever no trailing comma is present, so the clean path is byte-identical and
  zero-allocation — a caller retries a strict-parse failure on the stripped view
  with zero risk of altering behavior on well-formed output.
- **Grounded-capable wins.** Among balanced candidates, an object carrying a
  fact with a non-empty `source_episode_id` is preferred over a source-less
  object the reliability gate would quarantine wholesale.

## Strict rejection — what still fails

Recovery never softens the failure boundary. These inputs remain an explicit,
retry-eligible `Err` — never a hollow `Ok`:

| Input | Result | Why |
| --- | --- | --- |
| **Empty / whitespace-only** facts document | `Err` (`ParseFailure`) | The agent produced no output; a hollow `Ok` would silently mark episodes distilled and lose them. |
| **No `{ "facts": [...] }` object** present (banner-only, pure prose) | `Err` (`ParseFailure`) | Nothing parseable; retried with JSON-format reinforcement. |
| **Genuinely malformed JSON** (elided element `[1,,2]`, unquoted key, missing value) | `Err` (`ParseFailure`) | The trailing-comma repair leaves non-trailing-comma malformations unchanged, so strict `serde` still rejects them. |
| **Missing facts file** even when stdout carries a facts object | `Err` (`ParseFailure`) | Stdout is never scraped as a backup — a silent fallback is a silent failure. |
| **Non-zero recipe exit** | `Err` (`CopilotTerminalFailure`) | The recipe process failed before parsing; surfaced with truncated stderr/stdout. |

Error messages reuse `truncate(…, 200)`, so a large or hostile document never
echoes its full content into a log or a metric line.

## Zero-facts disposition

A **successful** parse can still yield **zero promoted facts**, and there are two
operationally distinct reasons for that. Before this hardening both looked
identical in the metrics (`outcome=success, fact_count=0`), so an operator
watching only `distill_success_rate` could not tell them apart without manually
joining to `distill_reliability_gate`. The finished state makes the reason a
**first-class field** on the success metric's context.

> **Implementation status — shipped.** Like the recovery tiers, strict
> rejection, and metrics above, `zero_facts_reason` and its `candidate_facts`
> input are now in the code at this branch's HEAD.
> `build_distill_success_context` takes `candidate_facts` (threaded after
> `fact_count`) and emits `zero_facts_reason` on the success context.

`build_distill_success_context` emits a `zero_facts_reason` computed from data
already in scope at the call site — the count of **candidate** facts (parsed,
pre-gate) and the count of **promoted** facts (`fact_count`, post-gate):

| `zero_facts_reason` | Condition | Operator meaning |
| --- | --- | --- |
| `"none"` | `fact_count > 0`, **or** the pass was a parse/recipe failure | Facts were promoted (or there was no successful parse to classify). Nothing to investigate. |
| `"true_empty"` | success, `fact_count == 0` **and** `candidate_facts == 0` | The agent returned a valid `{ "facts": [] }` — **nothing worth distilling**. Re-running would produce the same empty result; this is a correct, non-actionable success. |
| `"all_quarantined"` | success, `fact_count == 0` **and** `candidate_facts > 0` | The agent produced facts but the **reliability gate blocked every one** (ungrounded provenance, empty content, off-spec concept). Actionable: inspect the gate block-rate and the distiller prompt. |

Design constraints of the disposition:

- **Additive only.** `zero_facts_reason` is a new *context* field on the existing
  `distill_success_rate` event. The metric `value` (`1.0` success / `0.0`
  failure), the denominators, and the success semantics are unchanged, so no
  existing dashboard or downstream reader breaks.
- **`all_quarantined` is still a success.** An all-quarantined pass is a genuine,
  retry-pointless success — re-running yields the same quarantine. It is **not**
  reclassified as a parse failure; doing so would inflate the failure rate with a
  non-parse condition and cause pointless retries.
- **Cross-check remains valid.** The same distinction is independently derivable
  by joining `distill_reliability_gate` (`candidate_facts`, `promoted`,
  `block_rate`) to `distill_success_rate`; `zero_facts_reason` makes it a
  single-event read instead of a join.

## Metrics

All three distill reliability metrics are appended to
`~/.simard/metrics/metrics.jsonl` via `self_metrics::record_metric`. Each is
**best-effort** (a write failure is logged, never propagated) and a **no-op
under `cfg!(test)`** so unit tests never touch the operator's real metrics file.
The JSON key for the metric name is `metric_name` (the `MetricEntry` field name);
the structured counters live inside the stringified `context` field.

### `distill_parse_success_rate` — the headline rate

Emitted **only** for passes that reached output parsing (`parse_attempted ==
true`): every success, plus a `ParseFailure`. Recipe-reported / terminal / spawn
/ serialize failures never reached parsing and are excluded from the denominator
(they emit no event). Therefore the **plain mean** of `distill_parse_success_rate`
values over a window is exactly the parse-success rate this hardening drives
toward `1.0`.

- `value`: `1.0` on a parse success, `0.0` on a `ParseFailure`.

### `distill_success_rate` — every pass that ran the recipe

Emitted for every pass that ran the recipe (success **or** a recipe/parse
failure); below-threshold skips are excluded. Its context is the superset payload
shared with `distill_parse_success_rate`:

| Context field | Type | Meaning |
| --- | --- | --- |
| `outcome` | `"success"` \| `"failure"` | Pass outcome. |
| `recipe_exited_ok` | bool | The recipe process exited `0`. |
| `parse_attempted` | bool | A step ran and its output was parsed. |
| `parse_success` | bool | Parsing yielded a facts object. |
| `failure_class` | string \| null | One of `spawn-failure`, `copilot-terminal-failure`, `recipe-reported-failure`, `parse-failure`, `serialize-failure`, `other`. |
| `input_count` | u32 | Episodes fed to the pass. |
| `fact_count` | u32 | Facts **promoted** (`0` on failure or all-quarantined). |
| `candidate_facts` | u32 | Facts **parsed** before the reliability gate. |
| `zero_facts_reason` | `"none"` \| `"true_empty"` \| `"all_quarantined"` | The zero-facts disposition (see [above](#zero-facts-disposition)). |
| `attempt` | u32 | 1-based runner invocation count for the pass. |
| `recovered_after_retry` | bool | Success followed at least one in-cycle retry. |

The context is built with `serde_json::json!` and serialized, so no raw agent
substring, ANSI byte, or un-escaped newline can leak into a metrics line —
`metrics.jsonl` is line-oriented and may be world-readable, so it must carry only
low-cardinality classification counters.

### `distill_reliability_gate` — the block-rate

Emitted once per pass that promoted facts. `value` is the block-rate fraction
`quarantined / candidate_facts` (`0.0` when `candidate_facts == 0`). Context:
`candidate_facts`, `promoted`, `quarantined`, `block_rate`, `threshold`. This is
the metric `zero_facts_reason=all_quarantined` points an operator at.

## Configuration

The parse-resilience contract itself has **no configuration knobs** — recovery
and strict rejection are always on, because they are correctness properties, not
tunables. The surrounding pass and its diagnostic *are* configurable:

| Variable | Default | Purpose |
| --- | --- | --- |
| `SIMARD_DISTILL_MIN_EPISODES` | see [scheduler API](./automatic-distillation-scheduler.md) | Below this batch size the pass skips the LLM call entirely (no metric emitted). |
| `SIMARD_DISTILL_INTERVAL_CYCLES` | see [scheduler API](./automatic-distillation-scheduler.md) | Cycles between automatic distillation passes. |
| `SIMARD_DISTILL_RAW_CAPTURE` | `0` (off) | Persist the raw output of a *residual* `ParseFailure` for harvesting into a regression test — see [raw-capture reference](./distill-raw-capture-on-parse-failure.md). |
| `SIMARD_STATE_ROOT` | `$HOME/.simard` | Root under which `metrics/metrics.jsonl` (and captures) live. |

The in-cycle retry bound (`DISTILL_PARSE_RETRY_MAX`) and the reliability
threshold (`DISTILL_RELIABILITY_THRESHOLD`) are compile-time constants, not
environment variables.

## Public API

The parse-resilience surface is `pub(crate)`; the trailing-comma repair is a
`pub` helper in the shared `recipe_output` module so any recipe-backed parser can
reuse it.

```rust
// src/memory_consolidation/distillation.rs

/// Parse the distill agent's facts document (the contents of the dedicated
/// facts file) into facts AND procedures. Empty or answerless input is an
/// explicit `Err`; a fenced / prose-wrapped / single-trailing-comma /
/// off-spec-field document is recovered.
pub(crate) fn parse_facts_document(document: &str) -> SimardResult<DistillOutput>;

/// Facts-only wrapper over `parse_facts_document`, retained for the legacy
/// `DistillRecipeRunner::run` entry point.
pub(crate) fn parse_facts(document: &str) -> SimardResult<Vec<DistilledFact>>;
```

```rust
// src/recipe_output/extract.rs

/// Strip JSON trailing commas — a `,` immediately preceding a closing `}`/`]`
/// (ignoring intervening ASCII whitespace) that is OUTSIDE a JSON string
/// literal. A provable no-op on valid input: returns `Cow::Borrowed`
/// byte-for-byte unchanged when no trailing comma is present. String-aware, so
/// a comma inside a fact's `content` is never touched.
pub fn strip_json_trailing_commas(s: &str) -> std::borrow::Cow<'_, str>;

/// Every balanced top-level `{…}` span in `s`, string-aware (a brace inside a
/// JSON string cannot split an object) and resilient to an unmatched `{`.
pub fn balanced_objects(s: &str) -> Vec<&str>;
```

The success-metric context builder takes the disposition inputs (`candidate_facts`
is threaded after `fact_count`):

```rust
// src/memory_consolidation/distillation.rs

/// Build the JSON `context` for `distill_success_rate` / `distill_parse_success_rate`.
/// `candidate_facts` (parsed, pre-gate) and `fact_count` (promoted, post-gate)
/// together determine `zero_facts_reason`.
fn build_distill_success_context(
    success: bool,
    class: Option<DistillFailureClass>,
    input_count: u32,
    fact_count: u32,       // promoted
    candidate_facts: u32,  // parsed, before the reliability gate
    attempt: u32,
    recovered_after_retry: bool,
) -> String;
```

## Examples

### A single trailing comma is recovered

The agent emits an otherwise-perfect envelope with one trailing comma before the
closing `]`:

```json
{ "facts": [
  { "concept": "pr-pattern", "content": "warm the shared cache before pin bumps", "source_episode_id": "epi_1" },
] }
```

Strict `serde` rejects it; the `strip_json_trailing_commas` retry removes the
lone comma and the parse succeeds — one fact promoted. (This pass carries
`zero_facts_reason=none`.)

### Nothing worth distilling (`true_empty`)

The agent returns a valid empty envelope:

```json
{ "facts": [], "procedures": [] }
```

This is a **success** with zero promoted facts and zero candidates, so
`fact_count=0`, `candidate_facts=0`, `zero_facts_reason="true_empty"`. No action
needed.

### The reliability gate blocked everything (`all_quarantined`)

The agent returns three facts, all with an out-of-batch `source_episode_id`
(hallucinated provenance). Each parses but is quarantined by the reliability
gate:

```json
{ "facts": [
  { "concept": "pr-pattern", "content": "always rebase before merge", "source_episode_id": "not_in_batch" }
] }
```

The pass is a **success** but promotes nothing: `fact_count=0`,
`candidate_facts=1`, `zero_facts_reason="all_quarantined"`. This is the
actionable signal — inspect `distill_reliability_gate` and the distiller prompt.

### A genuinely malformed object still fails

```json
{ "facts": [1,,2] }
```

The trailing-comma repair leaves an *elided element* unchanged, so strict `serde`
still rejects it — an explicit `ParseFailure`, retried with JSON reinforcement.
Precision is not weakened.

## Tutorial — measure the before/after rate

The parse-success rate is directly readable from `metrics.jsonl`, so you can
confirm the hardening worked without re-running a live pass.

1. **Read the recent parse-success events.** Each pass that reached parsing emits
   one:

   ```bash
   grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
     | tail -n 50
   ```

2. **Compute the rate.** The plain mean of the `value` fields is the
   parse-success rate. A healthy deployment sits at or near `1.0`; the pre-fix
   spike sat near `0.0`:

   ```bash
   grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
     | grep -o '"value":[0-9.]*' | cut -d: -f2 \
     | awk '{ s += $1; n += 1 } END { if (n) printf "parse-success rate = %.3f over %d passes\n", s/n, n }'
   ```

3. **Split zero-fact successes** *(via `zero_facts_reason`).*
   When the rate is `1.0` but memory is not growing, read the disposition to tell
   *nothing to learn* from *everything blocked*:

   ```bash
   grep '"metric_name":"distill_success_rate"' ~/.simard/metrics/metrics.jsonl \
     | grep -o '"zero_facts_reason\\":\\"[a-z_]*\\"' | sort | uniq -c
   ```

   A run of `all_quarantined` points at the reliability gate / distiller prompt;
   a run of `true_empty` means the episode stream genuinely has nothing worth
   distilling.

4. **If a residual `ParseFailure` persists,** harvest the exact failing bytes
   with the env-gated diagnostic and turn them into a regression fixture — see
   [Capture and diagnose a failing distill sample](../howto/capture-and-diagnose-a-failing-distill-sample.md).

## Testing

The contract is pinned by `distillation_tests.rs`, the zero-facts unit tests in
`distillation.rs`, and the hermetic `issue_2622_file_channel_tests`.
Representative cases:

| Test | Asserts |
| --- | --- |
| `parse_recovers_bare_trailing_comma_facts_object` | A single trailing comma is recovered (tier 3). |
| `fenced_facts_document_still_parses` / `document_tolerates_prose_and_fence` | Code-fence / prose wrapper is recovered (tier 1). |
| `launcher_banner_on_stdout_does_not_cause_parse_failure` | A banner on stdout + a valid facts file ⇒ success (banner immunity). |
| `missing_facts_file_is_parse_failure_never_stdout_fallback` | A missing file ⇒ `ParseFailure` even when stdout holds a facts object. |
| `empty_facts_document_is_parse_failure` / `banner_only_document_is_parse_failure` | Empty / answerless input errors explicitly. |
| `nonzero_exit_is_terminal_failure_with_context` | A non-zero exit ⇒ `CopilotTerminalFailure`. |
| `document_drops_unknown_concepts` | The concept allow-list still applies to recovered facts. |
| `document_error_does_not_leak_full_payload` (50 KB) / `document_tolerates_deeply_nested_input_without_panic` | Bounded, panic-free error output. |
| `document_handles_large_valid_input` (1 000 facts) | A large valid document extracts every fact; repair stays O(n). |
| `zero_facts_reason_*` / `success_context_*` zero-facts tests | `zero_facts_reason` is `none` / `true_empty` / `all_quarantined` for the three input shapes, gated on `success`. |
| `strip_trailing_commas_valid_json_is_borrowed_zero_copy` / `strip_trailing_commas_never_corrupts_comma_in_string_content` | The repair borrows valid JSON unchanged and never touches an in-string comma. |

Run the suite:

```bash
cargo test --lib memory_consolidation::distillation
```

## Non-goals

- **No new parser or leniency tier** beyond a single trailing comma — widening
  recovery further would weaken precision (the strict-rejection boundary above).
- **No stdout re-scraping.** The facts-file channel is the structural banner fix;
  reintroducing stdout parsing would reopen the #2622 failure mode.
- **No reclassification of `all_quarantined` as a failure** — it is a correct,
  retry-pointless success.
- **No rename or removal** of `distill_success_rate`, `distill_parse_success_rate`,
  or `distill_reliability_gate` — the `zero_facts_reason` field is purely
  additive.

## Related

- [Distill recipe output capture](./distill-recipe-output-capture.md) — the
  facts-file capture contract this hardening builds on.
- [Distill raw-capture on parse failure](./distill-raw-capture-on-parse-failure.md)
  — the env-gated diagnostic for harvesting a residual failure.
- [Capture and diagnose a failing distill sample](../howto/capture-and-diagnose-a-failing-distill-sample.md)
  — the step-by-step harvesting how-to.
- [Telemetry metrics reference](./telemetry-metrics.md) — the unified telemetry
  facade and the `simard.distill.*` OTel counter set.
- [Automatic distillation scheduler API](./automatic-distillation-scheduler.md)
  — when the pass fires and the threshold gate.
- [Episode distillation](../architecture/episode-distillation.md) — the
  surrounding pipeline.
