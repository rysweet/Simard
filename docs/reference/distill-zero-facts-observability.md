---
title: Distill zero-facts observability & parse-failure-rate recovery
description: How Simard's distillation pass makes a "valid parse that yielded zero usable facts" observable without counting it as a parse failure — the content-free zero-facts log emitted from RecipeEnvelope::into_facts, why it distinguishes off-spec-concept contract drift from a legitimately empty envelope, the single-fire guarantee, and the deterministic rate-verification harness that pins the residual 74%-era parse-failure rate to 0 across all three historical failure shapes (issue #2689).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./distill-recipe-output-capture.md
  - ./telemetry-metrics.md
  - ../architecture/episode-distillation.md
  - ../howto/capture-and-diagnose-a-failing-distill-sample.md
  - ./distill-raw-capture-on-parse-failure.md
  - ../memory.md
---

# Distill zero-facts observability & parse-failure-rate recovery

> **Status — implements the
> [#2689](https://github.com/rysweet/Simard/issues/2689) fix.** Present tense
> below describes the shipped behavior. Locations:
> `RecipeEnvelope::into_facts` and the rate-verification harness in
> `src/memory_consolidation/distillation.rs`; the deterministic benchmark in
> `src/memory_consolidation/distillation_fact_yield_bench.rs`;
> tests in `distillation.rs`'s `unit_tests` module and
> `src/memory_consolidation/distillation_tests.rs`.

The episode-distillation pass turns batches of episodic memory into semantic
**facts** by shelling out to `recipe-runner-rs` and reading the distill agent's
JSON output. Historically the **distill parse-failure rate sat near 74%**: most
passes exited `0`, produced structurally-parseable JSON, and still stored zero
facts — so the batch was deferred every cycle, an LLM call was burned, and
memory fact-yield stayed near zero.

Three distinct root causes produced that one symptom. Two were already
structurally eliminated:

1. **Launcher-banner stdout contamination** — fixed by the dedicated facts-file
   channel (issues [#2622](https://github.com/rysweet/Simard/issues/2622) /
   [#2619](https://github.com/rysweet/Simard/issues/2619); see
   [Distill recipe output capture](./distill-recipe-output-capture.md)).
2. **A single trailing comma in the agent's JSON** — fixed by the tolerant
   parser (`parse_facts_envelope_lenient` +
   `recipe_output::strip_json_trailing_commas`, issue
   [#2658](https://github.com/rysweet/Simard/issues/2658)).

The **third** cause was invisible: a document that parsed cleanly but whose
facts were **all dropped as off-spec concept labels** by
[`RecipeEnvelope::into_facts`](../architecture/episode-distillation.md). That
pass recorded `distill_parse_success_rate = 1.0` (parsing succeeded) yet stored
nothing, so the residual failure had no signal an operator could see.

This page documents the two pieces #2689 adds to close that gap:

- a **content-free zero-facts observability log** that makes a valid-but-empty
  distill result visible and — critically — distinguishes **contract drift**
  (all facts dropped as off-spec concepts) from a **legitimately empty**
  `{"facts":[]}` envelope, and
- a **deterministic rate-verification harness** that reproduces all three
  historical 74%-era failure shapes and asserts each now recovers.

Neither piece changes what facts are stored: a well-formed document produces a
byte-identical `DistillOutput`.

---

## The zero-facts observability signal

`RecipeEnvelope::into_facts` filters the recipe's facts through the concept
allow-list (`canonical_distill_concept`, three labels:
`pr-pattern` / `bug-pattern` / `lesson-learned`, with surface-form
canonicalization). It now measures the filter's effect and emits a distinct log
**only when the surviving fact list is empty**:

| `pre_filter` (facts in) | `post_filter` (facts out) | Signal | Level | Meaning |
|---|---|---|---|---|
| `> 0` | `0` | **loud** | `info!` + `eprintln!` | **Contract drift** — the agent produced facts but *every one* was dropped as an off-spec concept label. This is the residual 74%-era shape made visible. |
| `0` | `0` | **quiet** | `trace!` | **Legitimately empty** — the agent returned `{"facts":[]}` ("nothing worth distilling"). Expected; not noteworthy. |
| any | `> 0` | *(none)* | — | Normal success path; no zero-facts log. |

- **Counts only.** The message carries the integer `pre_filter` (and the fact
  that `post_filter == 0`) — **never** fact `content`, `concept`, or
  `source_episode_id`. See [Security & privacy](#security--privacy).
- **Distinct, non-error message.** The loud line is a stable, non-error
  message that is deliberately disjoint from every `parse-failure` /
  `SimardError` distill string, so `classify_distill_error` and the
  parse-rate metric never mistake it for a failure. `into_facts` never returns
  `Err`.
- **Return value unchanged.** The log is a pure side effect; the returned
  `Vec<DistilledFact>` is identical to the pre-#2689 behavior.

### Message shape

```text
# pre_filter > 0 — contract drift (loud, info-level)
[simard] distill: valid parse yielded zero usable facts: all 4 fact(s) dropped as off-spec concepts

# pre_filter == 0 — legitimately empty (quiet, trace-level)
# (only visible at RUST_LOG trace)
distill: valid parse with an empty facts list (nothing worth distilling)
```

Both emit under the `simard::distill` tracing target. The loud line follows the
module's dual-write convention (`tracing::info!` **and** a `[simard] distill:`
`eprintln!`) so it is visible in journald and on an attached terminal alike; the
quiet line is `trace!`-only.

### Single-fire guarantee

The signal fires **exactly once per parsed document**. `into_output` constructs
the `DistillOutput.facts` list by routing the envelope's `facts` through a
facts-only `RecipeEnvelope` into `into_facts`, and the envelope's `procedures`
through a separate procedures-only `RecipeEnvelope` into `into_procedures`. The
procedures branch never calls `into_facts`, so a document is never
double-counted, and the zero-facts log cannot be emitted twice for one parse.

---

## Relationship to `distill_parse_success_rate`

A zero-facts result is a **parse success**, not a parse failure — and the
telemetry reflects that, unchanged:

- `distill_parse_success_rate` (the `self_metrics` JSONL rate) and
  `simard.distill.runs{result="ok"}` (the telemetry-facade counter) both record
  **success** for a valid-but-empty pass. The stored `fact_count` is `0`.
- The zero-facts log is **complementary observability**, not a metric mutation:
  it explains *why* an `ok` pass stored nothing, without moving the pass into
  the `parse_fail` bucket.

This separation is the point. `distill_parse_success_rate` answers *"did the
output parse?"*; the zero-facts log answers *"did a clean parse still yield
nothing, and was that contract drift or an honestly empty batch?"* — the exact
question the invisible 74%-era failures could not previously answer.

For the full metric catalog, the `parse_attempted` / `parse_success`
denominator gate, and the retry accounting, see
[Distill recipe output capture → Failure semantics](./distill-recipe-output-capture.md#failure-semantics)
and the [Telemetry metrics reference](./telemetry-metrics.md).

---

## Rate-verification harness

Recovery of the three historical 74%-era shapes is pinned by a mix of
**already-merged** deterministic benchmarks — which are **verify-only** and must
**not** be re-created — plus **one new** combined harness that #2689 adds. All
run the shapes through the **real production entry point**,
`parse_facts_document`, and treat a pass as a "failure" iff it returns `Err`
**or** yields zero facts:

| Historical shape (74%-era) | Fixed by | Pinned by (status) |
|---|---|---|
| Single **trailing comma** before `}` / `]` | #2658 tolerant parser (`parse_facts_envelope_lenient` + `recipe_output::strip_json_trailing_commas`) | `distill_parse_failure_rate_benchmark_before_1000_after_0000` in `distillation_fact_yield_bench.rs` — **already merged (#2658); verify-only** (strict baseline `1.000` → shipped `0.000`) |
| **Leading launcher banner** / prose before the JSON object | banner-agnostic balanced-object scan (`recipe_output::balanced_objects`) | `document_tolerates_prose_and_fence` in `distillation_tests.rs` (trailing balanced object selected; grounded fact yielded) — **already merged; verify-only** |
| **Off-spec concept labels** (surface-form variants) | `canonical_distill_concept` canonicalization | `fact_yield_benchmark_recovers_only_surface_variants_not_offspec` in `distillation_fact_yield_bench.rs` — **already merged; verify-only** (variants canonicalize and store; off-spec still dropped) |

#2689 adds **one** new integration test,
`distill_recovers_trailing_comma_banner_and_offspec_shapes` (in
`distillation_tests.rs`), that drives all three shapes through
`parse_facts_document` in a single place — a consolidated regression guard that
proves no shape has regressed. It **reuses** the merged recovery primitives and
does **not** re-implement `strip_json_trailing_commas`, `balanced_objects`,
`canonical_distill_concept`, or the pre-existing benchmarks above (ruthless
simplicity; the design spec's primary risk is duplicating merged work).

These benchmarks are the deterministic analog of the live
`distill_parse_success_rate` self-metric: the metric is driven from the same
`parse_facts_document` return value on every real pass, so a production distill
pass over any of these shapes moves the live metric off the floor by the exact
mechanism the harness proves in-process. If any shape regresses, CI fails
**before** it can silently depress the live rate again.

---

## Examples

### Example 1 — contract drift (loud signal)

The agent returns three facts, but labels them `"pull-request"`,
`"pull-request"`, `"made-up-label"` — none canonicalize to an allowed concept.
The parse succeeds; `into_facts` returns an empty vec and emits:

```text
[simard] distill: valid parse yielded zero usable facts: all 3 fact(s) dropped as off-spec concepts
```

`distill_parse_success_rate` records **success** (the output parsed);
`fact_count = 0`. The loud line tells the operator the recipe prompt has drifted
from the concept contract — actionable without leaking the offending labels.

### Example 2 — legitimately empty (quiet signal)

The agent returns `{"facts":[],"procedures":[]}` for a low-value batch. The
parse succeeds, no facts are stored, and only a `trace!`-level note is emitted
(invisible at the default log level). This is the expected "nothing worth
distilling" outcome, not a defect.

### Example 3 — normal success (no signal)

The agent returns one grounded `pr-pattern` fact. `into_facts` returns a
non-empty vec; **no** zero-facts log is emitted, and the fact flows to the
reliability gate as usual.

---

## Configuration

No new configuration is introduced.

- The **loud** (`info!`) contract-drift line is visible at the daemon's default
  log level and in journald.
- The **quiet** (`trace!`) legitimately-empty note is suppressed by default;
  raise the `simard::distill` target to `trace` (e.g.
  `RUST_LOG=simard::distill=trace`) to see it while diagnosing.
- To harvest the *bytes* behind a contract-drift pass into a regression test,
  use the env-gated raw-capture diagnostic
  (`SIMARD_DISTILL_RAW_CAPTURE`); see
  [Capture and diagnose a failing distill sample](../howto/capture-and-diagnose-a-failing-distill-sample.md).

---

## Security & privacy

- **Content-free by construction.** The zero-facts log emits **only integer
  counts** (`pre_filter`, and the implicit `post_filter == 0`). Fact `content`,
  `concept` labels, and `source_episode_id` values are **never** written to
  stderr/journald. Emitting zero untrusted strings also eliminates any ANSI /
  newline log-injection surface for the new line.
- **Non-error signal integrity.** The message is a stable, distinct string that
  shares no leading prefix with any `SimardError` distill message, so
  `classify_distill_error`, the `parse_fail` telemetry bucket, and the
  raw-capture failure gate never treat it as a failure. `into_facts` returns
  `Err` in no case.
- **No new trust boundary or sink.** The signal reuses the existing
  `tracing` / `eprintln!` sinks only; it opens no file, network, or metrics
  path, and performs **no re-parsing** of untrusted bytes.
- **Existing input controls preserved.** The concept allow-list (three labels)
  is not relaxed, the strict-parse-first ordering is unchanged, and recovery
  remains bounded, single-pass, and panic-free.

---

## Code location

| Item | File |
|---|---|
| `RecipeEnvelope::into_facts` (zero-facts signal) | `src/memory_consolidation/distillation.rs` |
| `RecipeEnvelope::into_output` (single-fire routing) | `src/memory_consolidation/distillation.rs` |
| `canonical_distill_concept` (concept allow-list) | `src/memory_consolidation/distillation.rs` |
| `parse_facts_document` (production parse entry point) | `src/memory_consolidation/distillation.rs` |
| Rate-verification benchmark | `src/memory_consolidation/distillation_fact_yield_bench.rs` |
| Unit tests (`unit_tests` module) | `src/memory_consolidation/distillation.rs` |
| Document-level tests | `src/memory_consolidation/distillation_tests.rs` |

---

## Testing

The behavior is pinned by the in-file `unit_tests` module (private access to
`into_facts`), the document-parse tests, and the deterministic benchmarks.

**New with #2689** (the deliverables to add). The new `unit_tests` assert the
**logging side-effect** — the `into_facts` *filtering result* (surface-form
recovery, off-spec drop, empty vec) is **already** pinned by
`into_facts_recovers_surface_variant_but_drops_offspec` (see below) and must
not be re-asserted:

| Test | File | Coverage |
|---|---|---|
| `into_facts_logs_when_all_concepts_offspec` | `distillation.rs` `unit_tests` | facts present but all off-spec ⇒ empty vec + loud (`info`-level) signal emitted. |
| `into_facts_quiet_on_legitimately_empty_envelope` | `distillation.rs` `unit_tests` | `{"facts":[]}` ⇒ empty vec, no loud signal (trace-only note). |
| `into_facts_preserves_valid_facts_unchanged` | `distillation.rs` `unit_tests` | valid facts ⇒ byte-identical vec, **no** zero-facts signal (clean-path no-op regression). |
| `zero_facts_signal_is_content_free` | `distillation.rs` `unit_tests` | the emitted message contains no fact `content` / `concept` / `source_episode_id` substring (privacy defense-in-depth). |
| `into_facts_fires_once_per_document` | `distillation.rs` `unit_tests` | `into_output` routes facts through `into_facts` exactly once; no double-count. |
| `distill_recovers_trailing_comma_banner_and_offspec_shapes` | `distillation_tests.rs` | all three historical 74%-era shapes recover through `parse_facts_document` in one consolidated guard. |

**Pre-existing — verify-only** (already merged; #2689 must **not** re-create these):

| Test | File | Already covers |
|---|---|---|
| `into_facts_recovers_surface_variant_but_drops_offspec` | `distillation.rs` `unit_tests` | the `into_facts` **filtering result**: surface-form variants canonicalize and are kept; off-spec labels are dropped. The new tests build on this — they add only the log-emission assertions. |
| `distill_parse_failure_rate_benchmark_before_1000_after_0000` | `distillation_fact_yield_bench.rs` | trailing-comma batch: strict baseline `1.000`, shipped parser `0.000` (#2658). |
| `fact_yield_benchmark_recovers_only_surface_variants_not_offspec` | `distillation_fact_yield_bench.rs` | off-spec / surface-variant concept recovery is precision-safe. |
| `document_tolerates_prose_and_fence`, `empty_facts_envelope_is_success_not_failure` | `distillation_tests.rs` | banner/prose-before-JSON recovery; empty envelope = success, not failure. |

Run the distillation suite:

```bash
cargo test memory_consolidation::distillation
```

---

## Out of scope

- **Recipe / prompt contract changes.** #2689 makes the concept-contract drift
  *visible*; tightening the recipe prompt so the agent stops emitting off-spec
  labels is a separate, prompt-side follow-up.
- **A general JSON5 / lenient parser.** Recovery is limited to the single
  trailing-comma shape already shipped in #2658; no broader leniency is added.
- **Metric taxonomy redesign.** `distill_success_rate` /
  `distill_parse_success_rate` and the `simard.distill.*` facade counters are
  unchanged; the zero-facts log complements them, it does not replace them.
- **A shared parse-helper crate module.** The signal lives inline in
  `distillation.rs` (ruthless simplicity); extracting a reusable
  `recipe_output::extract` module for OODA/brain reuse remains an explicit
  non-goal here.

---

## History

The distill parse-failure rate was tracked across a long series of fixes that
each removed one contamination source on the old **stdout-envelope** capture
path (#2401, #2461, #2496, #2504, #2512, #2517, #2570), culminating in the
structural **file-channel** replacement (#2622/#2619) and the **trailing-comma**
tolerance (#2658). After those, the rate's *reported* success climbed — but a
residual class of passes parsed cleanly and still stored nothing because their
concept labels were off-spec, and nothing surfaced it. #2689 closes that final
observability gap and locks the whole recovery in with a deterministic
rate-verification harness, so a regression in any of the three historical shapes
fails CI rather than silently returning the rate to 74%.
