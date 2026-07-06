---
title: Distill parse-failure recovery & zero-facts telemetry
description: How Simard's distillation pass drives the distill parse-failure rate toward zero — the string-aware trailing-comma tolerance that recovers otherwise-well-formed LLM JSON (issue #2658), the counts-only zero-facts telemetry that distinguishes a valid-but-empty answer ("empty-array") from an off-spec one ("all-facts-off-spec") so the parse-success metric is never corrupted (issue #2679), and the strict Ok(zero)-vs-Err contract that forbids hollow successes.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./distill-recipe-output-capture.md
  - ./distill-raw-capture-on-parse-failure.md
  - ./telemetry-metrics.md
  - ../architecture/episode-distillation.md
  - ./text-parsing-wire-formats.md
  - ../../src/memory_consolidation/distillation.rs
  - ../../src/recipe_output/extract.rs
---

# Distill parse-failure recovery & zero-facts telemetry

> **Status: implemented.** Two complementary mechanisms keep the distillation
> pass's parse-failure rate at ~0% for the known deviation classes and make the
> remaining "zero facts" outcomes observable:
>
> - **Trailing-comma tolerance** — `strip_json_trailing_commas`
>   ([`src/recipe_output/extract.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/extract.rs)),
>   wired into `parse_facts_envelope_lenient`
>   ([`src/memory_consolidation/distillation.rs`](https://github.com/rysweet/Simard/blob/main/src/memory_consolidation/distillation.rs)) —
>   issue [#2658](https://github.com/rysweet/Simard/issues/2658) /
>   [#2675](https://github.com/rysweet/Simard/pull/2675).
> - **Zero-facts telemetry** — a single counts-only `tracing::info!` emitted
>   at most once per **resolved document** from `parse_facts_document`, over the
>   winning envelope's counts — issue
>   [#2679](https://github.com/rysweet/Simard/issues/2679).
>
> **Implementation status:** Mechanism 1 (trailing-comma tolerance) is already on
> `main`. Mechanism 2 (zero-facts telemetry) **ships in this PR** — it moves the
> emission onto the resolved-document path in `parse_facts_document` and threads
> the winning envelope's input counts out of `into_output`; `into_output` on
> `main` does not yet emit. This page is the retcon spec for that change.

The episode-distillation pass shells out to `recipe-runner-rs`, reads the
distill agent's `{ "facts": [...], "procedures": [...] }` envelope from a
dedicated facts file, and parses it with `parse_facts_document`. Historically a
**single cosmetic defect** in that JSON — most commonly a trailing comma before
a `}` or `]` — made `serde_json` reject the whole envelope, so the entire batch
was silently dropped and the `distill_parse_success_rate` metric collapsed
toward zero (the observed 76% / 91% parse-failure shape).

This page documents the two shipped mechanisms that close that gap, the exact
success/failure contract they preserve, the telemetry they emit, and the
data-privacy guarantees around that telemetry.

For where the envelope is captured (the file channel), see
[Distill recipe output capture](./distill-recipe-output-capture.md). For the
surrounding pipeline (when the pass fires, storage, the reliability gate), see
[Episode distillation](../architecture/episode-distillation.md).

---

## The contract in one table

`parse_facts_document` resolves every agent answer to exactly one of three
states. There is **no hollow `Ok`**: an unparseable document is always an
explicit, retry-eligible `Err`.

| Agent output                                              | Result                          | Counts as parse-failure? | Zero-facts log |
|----------------------------------------------------------|---------------------------------|--------------------------|----------------|
| Well-formed facts (incl. one trailing comma)             | `Ok(DistillOutput { facts, .. })` | no                     | no             |
| `{ "facts": [], "procedures": [] }` (nothing to distill) | `Ok(DistillOutput::default())`  | no                       | yes — `empty-array` |
| Inputs present but **all** filtered (off-allow-list facts and/or gated procedures) | `Ok(DistillOutput::default())`  | no                       | yes — `all-facts-off-spec` |
| Empty document / no `{ "facts": [...] }` object          | `Err(RpcError)`                 | **yes**                  | no             |
| Genuinely malformed JSON (not a trailing comma)          | `Err(RpcError)`                 | **yes**                  | no             |

A valid-but-empty parse is a **success with zero facts**, never a parse
failure. That distinction is what the zero-facts telemetry makes observable and
what keeps `distill_parse_success_rate` honest.

---

## Mechanism 1 — trailing-comma tolerance

### What it does

A trailing comma before a closing `}` or `]` is the single most common
real-world LLM JSON defect and is **never valid JSON**. `parse_facts_document`
recovers it as a **last resort, after** a strict `serde_json` parse fails, so
the clean path is byte-for-byte unchanged.

### API

```rust
// src/recipe_output/extract.rs
pub fn strip_json_trailing_commas(s: &str) -> std::borrow::Cow<'_, str>;
```

- **Provable no-op on valid input.** When no string-aware trailing comma is
  present, it returns `Cow::Borrowed(s)` — the same bytes, zero allocation. A
  caller can therefore retry any strict-parse failure on this view without any
  risk of altering behaviour on well-formed output.
- **String-literal aware.** A comma inside a JSON string (respecting `\"`
  escapes) is never touched, so a comma in a fact's `content` — even a literal
  `,}` or `,]` — is preserved verbatim.
- **Narrow by design.** Only the offending comma bytes are dropped. A genuinely
  malformed object (an elided element `[1,,2]`, an unquoted key, a missing
  value) is left still-malformed so the caller's strict parse still rejects it.
  Leniency never widens to accept broken JSON, single-quoted strings, comments,
  or unquoted keys — and **no `json5`/lenient dependency is added**.

### How it is wired

`parse_facts_envelope_lenient` is the single funnel every envelope parse routes
through (both the fast whole-document path and the balanced-object scan in
`scan_cleaned_for_facts`):

```rust
// src/memory_consolidation/distillation.rs
fn parse_facts_envelope_lenient(text: &str) -> Option<RecipeEnvelope> {
    // 1. Strict parse first — the clean path is unchanged.
    if let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(text) {
        return Some(parsed);
    }
    // 2. Only an actually-stripped (Owned) view can parse where strict failed.
    //    A Borrowed result means strict already saw these exact bytes.
    match crate::recipe_output::strip_json_trailing_commas(text) {
        std::borrow::Cow::Owned(stripped) => {
            serde_json::from_str::<RecipeEnvelope>(&stripped).ok()
        }
        std::borrow::Cow::Borrowed(_) => None,
    }
}
```

Because the retry only runs on the `Owned` (actually-changed) view, a document
that was already going to fail is never re-parsed pointlessly, and precision is
never weakened.

---

## Mechanism 2 — zero-facts telemetry

### What it does

When a **valid** parse resolves to zero storable facts and zero storable
procedures, `parse_facts_document` emits **exactly one** counts-only log — on
the *resolved winning* envelope — so operators can distinguish two very
different "zero" outcomes that both used to look identical in the logs:

- **`empty-array`** — the agent correctly reported it had nothing worth
  distilling (`{ "facts": [], "procedures": [] }`); the winning envelope's
  inputs were zero.
- **`all-facts-off-spec`** — the agent produced input (facts and/or
  procedures) but **every one** was filtered out: facts whose concept is off
  the `{pr-pattern, bug-pattern, lesson-learned}` allow-list (or that lack the
  fields the reliability gate requires), and/or procedures dropped by
  `into_procedures` (unnamed, step-less, or source-less). This single
  discriminant is an umbrella for *"the winning envelope carried input, yet
  nothing was kept"* — including the facts-only, procedures-only, and mixed
  cases.

The first is healthy operation; the second is a prompt/agent-quality signal
worth watching. Neither is a parse failure.

### The emitted event

```text
INFO simard::distill: distill parse yielded zero storable facts
    input_facts=3 input_procedures=0 kept_facts=0 kept_procedures=0
    reason="all-facts-off-spec"
```

| Field              | Type          | Meaning                                                        |
|--------------------|---------------|----------------------------------------------------------------|
| `target`           | `&'static str`| Always `simard::distill`.                                      |
| Level              | —             | `INFO` (a valid-but-empty parse is expected operation).        |
| `input_facts`      | integer       | Count of `facts[]` entries in the **winning** envelope (pre-filter).    |
| `input_procedures` | integer       | Count of `procedures[]` entries in the **winning** envelope (pre-filter).|
| `kept_facts`       | integer       | Always `0` on this event.                                      |
| `kept_procedures`  | integer       | Always `0` on this event.                                      |
| `reason`           | `&'static str`| One of `"empty-array"` or `"all-facts-off-spec"`.              |

**Emission rules (guaranteed):**

1. The event fires **only** when the resolved document keeps zero facts and
   zero procedures (`kept_facts == 0 && kept_procedures == 0`).
2. It fires **at most once per resolved document**. The single emission site is
   `parse_facts_document`, *after* `scan_cleaned_for_facts` returns `Ok` — never
   inside the scan, `into_output`, `into_facts`, or `into_procedures`. This is
   what makes the guarantee true for the **slow path** (see below): the scan
   converts many candidate objects speculatively, but only the resolved winner
   is ever logged, exactly once.
3. The counts belong to the **winning envelope only**. The winner is carried as
   a `ResolvedFacts { output, input_facts, input_procedures }` value that
   captures the winning envelope's pre-filter input counts alongside the
   filtered output, so a losing candidate — e.g. a trailing empty
   `{"facts":[]}` that a grounded object outranks — never contributes counts and
   never triggers a log.
4. `reason` is a fixed `&'static str` discriminant chosen from the **winner's**
   input counts: `"empty-array"` when the winning envelope carried nothing
   (`input_facts == 0 && input_procedures == 0`), else `"all-facts-off-spec"`
   (inputs present, nothing kept — covering off-allow-list facts, gated
   procedures, or both).

### How it is wired

The naïve site — `RecipeEnvelope::into_output` — is **wrong**, because
`scan_cleaned_for_facts` calls `into_output` *speculatively, in a reverse loop
over every balanced-object candidate* (the banner/prose-wrapped "slow path").
Logging there would (a) fire multiple times for a document with several empty
candidates and (b) emit a false `empty-array` for a document that ultimately
**succeeds** — a trailing empty candidate would be logged *before* the grounded
winner is returned, telling telemetry "zero facts" for a batch that actually
stored facts. That is exactly the wrapped-output case this feature targets, so
the log must live on the resolved-result path instead.

The fix carries the winning envelope's pre-filter counts out of the scan in a
small `ResolvedFacts` value (leaving `into_output`'s signature unchanged) and
emits once at the resolved document:

```rust
// src/memory_consolidation/distillation.rs

/// The winning envelope resolved to its stored output, carrying that
/// envelope's pre-filter input counts so a zero-facts outcome can be labelled
/// without the scan itself logging.
struct ResolvedFacts {
    output: DistillOutput,
    input_facts: usize,       // facts[] before the concept allow-list
    input_procedures: usize,  // procedures[] before the source/steps gate
}

impl ResolvedFacts {
    fn from_envelope(env: RecipeEnvelope) -> Self {
        let input_facts = env.facts.len();
        let input_procedures = env.procedures.len();
        // into_output() — unchanged: returns just the filtered DistillOutput.
        ResolvedFacts { output: env.into_output(), input_facts, input_procedures }
    }

    fn log_if_zero_facts(&self) {
        if !self.output.facts.is_empty() || !self.output.procedures.is_empty() {
            return;
        }
        let reason = if self.input_facts == 0 && self.input_procedures == 0 {
            "empty-array"
        } else {
            "all-facts-off-spec"
        };
        tracing::info!(
            target: "simard::distill",
            input_facts = self.input_facts,
            input_procedures = self.input_procedures,
            kept_facts = 0, kept_procedures = 0, reason,
            "distill parse yielded zero storable facts"
        );
    }
}

// The scan stays silent; it just carries the WINNING candidate's counts.
fn scan_cleaned_for_facts(trimmed: &str) -> Option<ResolvedFacts> {
    // fast path + the three-tier reverse-loop, selecting one winner; no logging.
}

// The ONE emission site: fires at most once, on the resolved winner's counts.
pub(crate) fn parse_facts_document(document: &str) -> SimardResult<DistillOutput> {
    let trimmed = document.trim();
    if trimmed.is_empty() {
        return Err(SimardError::RpcError(
            "distill: facts document was empty; the agent produced no output".into(),
        ));
    }
    if let Some(resolved) = scan_cleaned_for_facts(trimmed) {
        resolved.log_if_zero_facts();
        return Ok(resolved.output);
    }
    Err(SimardError::RpcError(/* did not contain a parseable {…} */))
}
```

Because "once per `into_output` call" is replaced by "once per resolved
document," a wrapped document with three empty candidates and one grounded
winner logs **nothing**, and a wrapped document that truly resolves to empty
logs **exactly once** — the behaviour Examples 3–4 require.

### Data-privacy guarantees

Distilled memory may contain PII, so the zero-facts event is **counts-only**:

- **Integers and static strings only.** The event carries `input_facts`,
  `input_procedures`, `kept_facts`, `kept_procedures`, and a `&'static str`
  `reason`. It **never** carries a fact's `content`, `concept`,
  `source_episode_id`, a procedure `name` / `steps`, or any raw JSON.
- **No interpolated model output.** `reason` is a compile-time constant, not
  derived from agent text, so a crafted fact value cannot inject into the log.
- **`info!`, not `error!`.** A valid-but-empty parse is normal operation and is
  logged as such, so it does not inflate error dashboards.

These guarantees are pinned by a negative test asserting the event carries no
`content` / `source_episode_id` fields.

### Relationship to `distill_parse_success_rate`

The zero-facts event is **diagnostic only** — it is a `tracing` log, not a
metric, and it does **not** touch the `distill_parse_success_rate` denominator.
Because a valid-but-empty parse resolves to `Ok`, it is counted as a **parse
success** (as it always was); the log simply annotates *why* it produced zero
facts. See the
[Telemetry metrics reference](./telemetry-metrics.md) for the metric itself.

---

## Examples

### Example 1 — trailing comma is recovered

The agent's answer carries one trailing comma before the closing `]`. Strict
`serde_json` rejects it; the lenient retry strips the comma and the batch is
recovered:

```json
{ "facts": [ { "concept": "pr-pattern", "content": "warm the shared cache before pin bumps", "source_episode_id": "epi_1" }, ] }
```

⇒ `Ok(DistillOutput { facts: [ pr-pattern … ], procedures: [] })`. No parse
failure, no zero-facts log.

### Example 2 — a comma inside a string is preserved

Trailing-comma stripping is string-aware, so content that literally contains
`,}` is never altered:

```json
{ "facts": [ { "concept": "lesson-learned", "content": "close the brace, then run tests,}", "source_episode_id": "epi_2" } ] }
```

⇒ the `content` round-trips **verbatim**, including the trailing `,}` inside the
string.

### Example 3 — nothing worth distilling (`empty-array`)

```json
{ "facts": [], "procedures": [] }
```

⇒ `Ok(DistillOutput::default())` **and** one log:

```text
INFO simard::distill: distill parse yielded zero storable facts
    input_facts=0 input_procedures=0 kept_facts=0 kept_procedures=0 reason="empty-array"
```

### Example 4 — every input off-spec (`all-facts-off-spec`)

The agent emitted facts, but each concept is outside the allow-list (e.g.
`"random-thought"`), so all are dropped:

```json
{ "facts": [ { "concept": "random-thought", "content": "…", "source_episode_id": "epi_3" } ] }
```

⇒ `Ok(DistillOutput::default())` **and** one log:

```text
INFO simard::distill: distill parse yielded zero storable facts
    input_facts=1 input_procedures=0 kept_facts=0 kept_procedures=0 reason="all-facts-off-spec"
```

The same `all-facts-off-spec` discriminant covers a **procedures-only** batch
in which every procedure is gated out (unnamed, step-less, or source-less) — the
umbrella is "inputs present, nothing kept," e.g.
`input_facts=0 input_procedures=2 … reason="all-facts-off-spec"`.

### Example 4b — wrapped output with a trailing empty candidate (no false log)

The distill agent wrapped its grounded answer in prose and left a stray empty
object after it. The reverse-loop scan converts the trailing `{"facts":[]}`
speculatively **but never logs**; the grounded object wins, and because the log
lives on the resolved winner the batch is stored with **no** zero-facts log:

```text
…thinking… {"facts":[]}
{ "facts": [ { "concept": "bug-pattern", "content": "…", "source_episode_id": "epi_4" } ] }
```

⇒ `Ok(DistillOutput { facts: [ bug-pattern … ], .. })`, **zero** logs. (Logging
inside `into_output` would instead have emitted a spurious `empty-array` here —
the bug this design avoids.)

### Example 5 — unparseable stays an error (no hollow Ok)

An empty document, a launcher-banner-only document, or JSON that is malformed in
a way trailing-comma stripping cannot fix returns an explicit `Err` and emits
**no** zero-facts log. Operators should grep for **either** error string:

```text
# empty document (agent produced no output)
Err: distill: facts document was empty; the agent produced no output
# non-empty but no parseable envelope (banner-only / unrecoverable malformation)
Err: distill: facts document did not contain a parseable { "facts": [...] } object: …
```

This is classified `ParseFailure` and retried in-cycle; on final failure the
pass returns `Err` **without** marking any episode, so the batch stays fully
retry-eligible.

---

## Operating guidance

- **Watch the `reason` split, not the raw zero count.** A steady stream of
  `empty-array` is healthy (nothing to distill). A rising share of
  `all-facts-off-spec` means the agent is producing facts the allow-list or
  reliability gate rejects — a prompt/quality signal, not a parser bug.
- **A spike in `ParseFailure`** (see
  [Distill recipe output capture](./distill-recipe-output-capture.md)) is the
  metric to alert on for genuine parse breakage; the trailing-comma tolerance is
  what should keep it near zero.
- **To harvest a real currently-failing sample** for a regression test, use the
  env-gated diagnostic in
  [Distill raw-capture on parse failure](./distill-raw-capture-on-parse-failure.md).

---

## Code location

| Item                                      | File                                                 |
|-------------------------------------------|------------------------------------------------------|
| `strip_json_trailing_commas`              | `src/recipe_output/extract.rs`                       |
| `parse_facts_envelope_lenient`            | `src/memory_consolidation/distillation.rs`           |
| `parse_facts_document` (zero-facts log emission site) | `src/memory_consolidation/distillation.rs`   |
| `scan_cleaned_for_facts` (threads winner counts, no logging) | `src/memory_consolidation/distillation.rs` |
| `RecipeEnvelope::into_output` (returns pre-filter input counts) | `src/memory_consolidation/distillation.rs` |
| `RecipeEnvelope::into_facts` / `into_procedures` (allow-list & gates) | `src/memory_consolidation/distillation.rs` |
| Tests                                     | inline `#[cfg(test)]` in `src/memory_consolidation/distillation.rs`, `src/recipe_output/extract.rs` |

---

## Testing

| Test                                                        | Coverage                                                                              |
|-------------------------------------------------------------|---------------------------------------------------------------------------------------|
| trailing-comma envelope parses to expected facts            | A previously-failing trailing-comma answer now yields facts (headline recovery).      |
| string-safety round-trip                                    | A `content` containing `,}` / `,]` is preserved verbatim.                              |
| `strip_json_trailing_commas` borrows clean input            | Valid JSON returns `Cow::Borrowed` (zero-alloc no-op) — the clean path is unchanged.   |
| off-spec-only document ⇒ `Ok(zero)` + one `all-facts-off-spec` log | Facts present but all filtered → success with zero facts, single log.           |
| procedures-only all-gated ⇒ `Ok(zero)` + one `all-facts-off-spec` log | `input_procedures>0, input_facts=0`, none kept → umbrella reason, single log. |
| `{ "facts": [] }` ⇒ `Ok(zero)` + one `empty-array` log      | Valid-empty is a success, single log with the empty-array reason.                     |
| wrapped: grounded winner after a trailing empty candidate ⇒ **no** log | Slow-path scan converts the empty candidate speculatively but never logs; winner stored, zero logs (guards against the false `empty-array`). |
| wrapped: document resolves to empty ⇒ **exactly one** log   | Emission is once per resolved document even when several empty candidates are scanned. |
| zero-facts event carries no `content` / `source_episode_id` | Data-privacy guard: counts-only, no PII in the log.                                    |
| empty / banner-only / non-trailing-comma malformed ⇒ `Err`  | Tier-3 parse failure, no hollow `Ok`, no zero-facts log; both `Err` strings covered.  |

**Acceptance:** `cargo build` and
`cargo test memory_consolidation::distillation` pass; the trailing-comma
deviation class parses at 0% failure in the fixture suite.

---

## Out of scope

- **Broadening JSON leniency** beyond trailing commas (single-quoted strings,
  comments, unquoted keys, a `json5` dependency) — explicitly excluded to avoid
  masking genuinely malformed output.
- **Prompt / taxonomy changes**, the `distill_parse_success_rate` metric rename,
  the recipe-runner binary, goal-board logic, and kgpacks-rs / BGE parity.
- **Making the zero-facts log a metric** — it is diagnostic `tracing` only and
  does not alter the parse-success denominator.
