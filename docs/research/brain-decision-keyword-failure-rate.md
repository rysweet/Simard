---
title: "Research: Brain decision-keyword parse-failure rate"
description: Measured rate of the recipe-engineer-lifecycle-brain "no decision keyword found; defaulting to continue_skipping" failure mode, its dominant cause, a proposed brain_decision_parse_failure_rate metric, and one bounded remediation.
last_updated: 2026-06-26
review_schedule: as-needed
owner: simard
doc_type: research
related:
  - ../reference/ooda-brain-parse-failure-record.md
  - ../reference/ooda-brain-decision-protocol.md
  - ../reference/recipe-brain-api.md
  - ../howto/diagnose-brain-decision-parse-failures.md
  - ../howto/diagnose-decide-orient-parse-failures.md
---

# Research: Brain decision-keyword parse-failure rate

> **Scope (one bounded question).** *How often does the engineer-lifecycle
> brain emit the `recipe-engineer-lifecycle-brain: no decision keyword found
> in recipe output; defaulting to continue_skipping` default instead of a
> parsed decision, what is the dominant cause, and how should we make that
> rate measurable?* This is a **research artifact only** — it measures,
> diagnoses, and proposes. It changes no runtime code beyond the metric and
> remediation **proposals** in [§5](#5-proposed-instrumentation-metric) and
> [§6](#6-proposed-remediation-bounded).

## 1. TL;DR

* **Measured rate: ~100%.** Across the 1,159 cycle reports on this host
  (`cycle_1` … `cycle_1159`), the engineer-lifecycle brain was invoked **495**
  times and produced a successfully-parsed, non-default decision **0** times.
  **493** invocations returned output that did not begin with a recognized
  keyword and fell to the `continue_skipping` default
  (493 / 495 = **99.6%** of invocations; 493 / 493 = **100%** of invocations
  that returned parseable output). The remaining **2** were hard
  invocation failures (recipe killed / non-zero exit).
* **Top cause: empty-or-malformed recipe output** — the recipe ran to a
  zero exit but its stdout did not start with one of the six lifecycle
  keywords, so `parse_lifecycle_from_text` returned `default_continue_skipping`.
  This bucket is **493 / 495 (99.6%)** of invocations.
* **The real reliability problem is invisibility, not just the rate.** Unlike
  the `decide` and `orient` brains — which route every failed invocation
  through `record_parse_failure` (Issue
  [#1890](https://github.com/rysweet/Simard/issues/1890)) and emit a
  `brain_parse_failure` metric — the engineer-lifecycle path emits **no
  metric, no `brain_judgments` record, and captures no raw response**. Its
  only on-disk footprint is a rationale substring inside `outcomes[].detail`.
  The `BrainPhase` enum (`Act`, `Decide`, `Orient`) has **no lifecycle
  variant**, so the existing parse-failure record *structurally cannot*
  represent it.
* **Proposal:** add a `brain_decision_parse_failure_rate` derivation backed by
  a new per-invocation `brain_lifecycle_decision` metric (numerator +
  denominator + cause bucket in one stream), plus one bounded remediation:
  a **retry-on-empty-output guard** before defaulting.

## 2. Why this question

The default rationale string —

```
recipe-engineer-lifecycle-brain: no decision keyword found in recipe output; defaulting to continue_skipping
```

— recurs across recent OODA cycles and is a direct **brain-reliability**
signal: every occurrence is a cycle where the brain was *asked* to decide an
engineer's lifecycle (keep skipping, deprioritize, reclaim, file an issue,
block the goal) and instead produced nothing the parser could act on. Because
the default is `continue_skipping`, a stuck goal can stall for many cycles
while the system reports itself healthy — exactly the fail-open class that
Issues [#1711](https://github.com/rysweet/Simard/issues/1711) and
[#1890](https://github.com/rysweet/Simard/issues/1890) were opened to close
for the other two brains. The lifecycle brain was left out of that
instrumentation; this study quantifies the gap.

## 3. Method (reproducible)

**Data sources (read-only, on the daemon host):**

| Source | Path | Window |
|--------|------|--------|
| Cycle reports | `~/.simard/cycle_reports/cycle_*.json` | 1,159 files, `cycle_1`–`cycle_1159` |
| Metrics stream | `~/.simard/metrics/metrics.jsonl` | 2026-05-19 → 2026-06-26 (~38 days) |
| Daemon logs | `~/.simard/ooda.log`, `ooda-daemon.log` | 26,345 lines |

**Code path under study** (`src/ooda_brain/recipe_brain.rs`):

```rust
// decide_engineer_lifecycle()
if !output.status.success() {
    return Err(SimardError::AdapterInvocationFailed { .. }); // → "brain failure" (hard)
}
let raw = String::from_utf8(output.stdout)...;
Ok(parse_lifecycle_from_text(&raw))                          // → default on no keyword
```

```rust
// parse_lifecycle_from_text()  — first-word extraction
let trimmed = text.trim();
if trimmed.is_empty() { return default_continue_skipping(); }   // empty bucket
let first_word = trimmed.split_whitespace().next().unwrap_or("");
// ...match the 6 variants case-insensitively...
else { default_continue_skipping() }                            // malformed bucket
```

The default lands in a cycle report at `outcomes[].detail`:

```
brain: continue_skipping (recipe-engineer-lifecycle-brain: no decision keyword found in recipe output; defaulting to continue_skipping)
```

**Numerator / denominator queries.** Lifecycle invocations are the
`outcomes[].detail` strings beginning with `brain:` (parsed/default) or
`brain failure:` (hard error). The numerator is the subset whose detail
contains `no decision keyword found in recipe output`.

```bash
# numerator: parse-failure defaults (across all cycle reports)
grep -rl "no decision keyword found in recipe output" ~/.simard/cycle_reports/ | wc -l   # → 364 files
# denominator + bucketing: see the Python aggregation below
```

```python
import json, glob, re, collections
variants = collections.Counter(); defaults = 0; hard = 0
for fn in glob.glob('~/.simard/cycle_reports/cycle_*.json'.replace('~','/home/azureuser')):
    d = json.load(open(fn))
    for oc in d.get('outcomes', []):
        det = oc.get('detail', '') or ''
        if det.startswith('brain failure'):
            hard += 1
        elif 'brain:' in det:
            m = re.search(r'brain:\s*([a-z_]+)', det)
            variants[m.group(1) if m else '?'] += 1
            if 'no decision keyword found in recipe output' in det:
                defaults += 1
# variants == {'continue_skipping': 493};  defaults == 493;  hard == 2
```

All counts in this document are reproducible from the snippets above against
the cycle-report corpus on the host at the time of writing.

## 4. Findings

### 4.1 The rate

| Quantity | Value |
|----------|-------|
| Lifecycle invocations recorded (`brain:` + `brain failure:` outcomes) | **495** |
| → parse-failure defaults (`no decision keyword found`) | **493** |
| → hard invocation failures (`Err`, recipe killed / exit≠0) | **2** |
| → successfully-parsed **non-default** decisions | **0** |
| Distinct cycle reports affected by the default | **364** |
| **Parse-failure default rate (defaults ÷ invocations)** | **493 / 495 = 99.6%** |
| **Parse-failure rate among parseable outputs (defaults ÷ non-error)** | **493 / 493 = 100%** |

> **Read this honestly.** A ~100% rate across every recorded invocation means
> this dataset reflects a **degraded or non-production brain configuration**
> (the recipe consistently emits output that does not lead with a lifecycle
> keyword — see §4.4). The *point* of the study is not "the brain fails 100%
> of the time in production"; it is that **when the lifecycle brain fails this
> way, nothing measures it**, so the same degradation could persist unnoticed
> in any environment. The other two brains show the same near-total fallback
> in this window (§4.3) — but they at least emit a metric.

### 4.2 Cause buckets

| Cause | Mechanism | Count | Share | Evidence |
|-------|-----------|------:|------:|----------|
| **Empty / malformed recipe output** | Recipe exits 0; stdout does not start with a lifecycle keyword (or is empty) → `default_continue_skipping` | **493** | 99.6% | `outcomes[].detail` default string in 364 cycle files |
| **SIGTERM / early-kill** (cf. [#2080](https://github.com/rysweet/Simard/issues/2080)) | Recipe `exited with signal: 15 (SIGTERM)` → `Err(AdapterInvocationFailed)` → "brain failure" | **1** | 0.2% | `cycle_196.json`: `base type 'recipe-engineer-lifecycle-brain' failed … signal: 15 (SIGTERM)` |
| **Non-zero exit** | Recipe `exited with exit status: 1` → `Err` → "brain failure" | **1** | 0.2% | `cycle_16.json`: `… exited with exit status: 1` |

**The empty-vs-malformed sub-split cannot be measured today.** Both paths in
`parse_lifecycle_from_text` collapse to the *same* default string, and — unlike
`decide`/`orient` — the lifecycle path **does not capture the raw response**
(`record_parse_failure` stores `raw_response_truncated`; the lifecycle path
stores nothing). So "empty body" vs "model emitted prose / wrong first word"
is unrecoverable from the cycle reports. That missing discriminator is the
single most valuable thing the proposed instrumentation would add.

**SIGTERM is real but rare for this brain.** Issue
[#2080](https://github.com/rysweet/Simard/issues/2080) (author `rysweet`,
verified, OPEN) documents a `disk-health-check` recipe killed by SIGTERM
during an OODA cycle; the daemon log shows 5 such SIGTERM events, four against
`disk-health-check` and one — `cycle_196` — against the
`recipe-engineer-lifecycle-brain` recipe itself. Early-kill therefore
contributes to the **hard-failure** bucket (the `Err` path), **not** the
`continue_skipping` default bucket. Conflating the two would misattribute the
dominant failure mode; they are distinct and counted separately above.

### 4.3 Baseline comparison (decide / orient are instrumented; lifecycle is not)

From the **same** cycle-report window (`brain_judgments[]` array):

| Phase | In `brain_judgments`? | In `metrics.jsonl`? | Fallback rate (cycle reports) | `brain_parse_failure` count (38-day metrics) |
|-------|:---:|:---:|---:|---:|
| `decide` | ✅ 1,781 records | ✅ | 1,781 / 1,781 = 100% | 12,139 |
| `orient` | ✅ 1,336 records | ✅ | 1,335 / 1,336 = 99.9% | 4,854 |
| **engineer-lifecycle** | ❌ **0 records** | ❌ **0 entries** | 493 / 495 = 99.6% (only via `outcomes[].detail`) | **0** |

The lifecycle brain is the **only** one of the three with no presence in
either telemetry channel. Its failures are equally frequent but **invisible**
to every existing dashboard query, which filters on `brain_judgments[].phase`
or on the `brain_parse_failure` metric.

### 4.4 Corroboration from the existing parse-failure contract

The decide/orient record contract already anticipates this gap. From
[`docs/reference/ooda-brain-parse-failure-record.md`](../reference/ooda-brain-parse-failure-record.md),
the `ParseFailureRecord.phase` field doc reads:

> `"decide"` or `"orient"`. Other phases (act, engineer-lifecycle) **have
> their own records**; this struct is only emitted by the two `_with_brain`
> call sites in `simard::ooda_loop`.

The engineer-lifecycle "own record" does **not exist yet** — that sentence
describes intended, not implemented, behavior. This study is the measurement
that motivates building it.

## 5. Proposed instrumentation metric

**Name:** `brain_decision_parse_failure_rate`

**Definition (derived):**

```
brain_decision_parse_failure_rate(window)
  = lifecycle_defaults(window) / lifecycle_invocations(window)
```

where a *default* is any lifecycle invocation that resolved to
`default_continue_skipping` (the "no decision keyword" path), and an
*invocation* is any call to `decide_engineer_lifecycle` (parsed, default, or
`Err`).

**Backing signal (single per-invocation event metric).** Rather than emit a
ratio (which cannot be aggregated across hosts), emit one event on **every**
lifecycle invocation and compute the rate downstream — this yields numerator,
denominator, **and** the cause bucket the cycle reports cannot currently
provide:

```rust
// Emitted once per decide_engineer_lifecycle() call, via the existing
// self_metrics::record_metric(name, value, context) sink (metrics.jsonl).
record_metric("brain_lifecycle_decision", 1.0, &json!({
    "goal_id": goal_id,
    // "parsed" | "default_empty" | "default_malformed" | "error_killed" | "error_exit"
    "outcome": outcome,
    "first_word": first_word_lossy,   // only when outcome starts with "default_"
    "consecutive_count": consecutive_count,
}).to_string());
```

Then:

```
brain_decision_parse_failure_rate
  = count(outcome LIKE 'default_%' OR outcome LIKE 'error_%')
    / count(*)            -- over brain_lifecycle_decision events in the window
```

**Why this shape (aligns with existing conventions):**

* Reuses `self_metrics::record_metric` and the `metrics.jsonl` sink already
  consumed by `operator_commands_dashboard::brain_failures` — no new storage,
  no new file format.
* Splits `default_empty` vs `default_malformed` at the emission point (the only
  place the empty-vs-prose distinction still exists), closing the §4.2 blind
  spot.
* Folds SIGTERM/early-kill (`error_killed`, cf. #2080) and non-zero exit
  (`error_exit`) into the same denominator so the dashboard cannot
  accidentally exclude the hard-failure bucket.
* Mirrors the four-channel philosophy of #1890 without forcing the lifecycle
  decision (which is not a `BrainJudgmentRecord`) into the decide/orient
  `ParseFailureRecord` struct.

**Dashboard surface:** add a third row to the brain-failures panel
(`src/operator_commands_dashboard/brain_failures.rs`) reporting
`brain_decision_parse_failure_rate` alongside the existing decide/orient
`brain_parse_failure` counts, so all three brains are visible in one read.

## 6. Proposed remediation (bounded)

**One change, scoped to the empty-output cause (the largest sub-bucket of the
dominant cause): a retry-on-empty-output guard in `decide_engineer_lifecycle`.**

```text
let raw = stdout;
if raw.trim().is_empty() {
    // bounded: exactly one retry, no backoff loop
    raw = rerun_recipe_once();         // same recipe, same ctx
    record_metric("brain_lifecycle_decision", 1.0, { outcome: "default_empty", retry_attempted: true, ... });
}
Ok(parse_lifecycle_from_text(&raw))
```

**Why this and not "loosen the parser":** the
[diagnose how-to](../howto/diagnose-brain-decision-parse-failures.md)
explicitly lists "editing the parser to *just accept* a new ad-hoc shape" as
an anti-pattern — if the model is not putting the variant first, the **prompt**
is wrong, not the parser. An empty body, by contrast, is a transient
invocation problem (adapter hiccup, truncated stdout) where a single retry is
a legitimate, bounded fix that does not weaken the protocol. A retry also
gives us a clean `retry_attempted` signal to confirm whether empties are
transient (retry succeeds) or systemic (retry also empty → prompt/adapter
work, tracked separately).

**Explicitly out of scope for this slice** (each is its own future PR):

* Adding an `EngineerLifecycle` variant to `BrainPhase` and routing the
  lifecycle default through `record_parse_failure` (the full four-channel
  treatment) — larger than a research slice; this doc only proposes it.
* Any prompt-asset change to `prompt_assets/simard/recipes/ooda-engineer-lifecycle.yaml`
  (and note: the deployed copy under `~/.simard/prompt_assets/` must never be
  edited directly — changes go via PR to `prompt_assets/` in this repo).
* Stricter keyword extraction or model/provider swaps.
* Fixing the SIGTERM early-kill in #2080.

## 7. Limitations & threats to validity

* **Single-host, single-window.** All counts come from one daemon's
  `cycle_reports/` and `metrics.jsonl`. The ~100% rate is specific to this
  (evidently degraded) environment and must not be read as a universal
  production figure. The methodology and the instrumentation gap, however,
  are environment-independent.
* **Denominator is outcome-derived, not call-counted.** Lifecycle invocations
  are inferred from `outcomes[].detail` strings, because the brain emits no
  dedicated record. If a cycle invoked the lifecycle brain but recorded no
  `brain:`/`brain failure:` outcome, it is invisible to this count — which is
  itself the gap this study documents. The proposed metric replaces inference
  with a direct per-invocation counter.
* **Empty-vs-malformed split is currently unmeasurable** (§4.2); the dominant
  bucket is reported as a single "empty/malformed" category until the proposed
  metric lands.

## 8. See also

* [How-to: Diagnose OODA Brain Decision Parse Failures](../howto/diagnose-brain-decision-parse-failures.md)
* [How-to: Diagnose decide/orient parse failures](../howto/diagnose-decide-orient-parse-failures.md)
* [Reference: OODA Brain Parse-Failure Record (#1890)](../reference/ooda-brain-parse-failure-record.md)
* [Reference: OODA Brain Decision Protocol](../reference/ooda-brain-decision-protocol.md)
* [Reference: RecipeBrain API](../reference/recipe-brain-api.md)
* Issue [#1711](https://github.com/rysweet/Simard/issues/1711) — no silent `continue_skipping` fallback on lifecycle brain `Err`.
* Issue [#1890](https://github.com/rysweet/Simard/issues/1890) — decide/orient parse-failure visibility.
* Issue [#2080](https://github.com/rysweet/Simard/issues/2080) — recipe killed by SIGTERM during an OODA cycle (early-kill cause bucket).
