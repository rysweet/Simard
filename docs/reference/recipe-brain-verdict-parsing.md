---
title: Recipe-brain verdict/decision parsing
description: Current recipe-backed brain parsing contract; OODA use becomes legacy only after verified typed-route cutover.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./recipe-brain-api.md
  - ./text-parsing-wire-formats.md
  - ./distill-recipe-output-capture.md
  - ./ooda-brain-parse-failure-record.md
  - ./pr-finalization-pipeline.md
  - ../concepts/copilot-launcher-preamble-stripping.md
  - ../howto/diagnose-merge-pr-verdict-parse-failures.md
  - ../howto/diagnose-decide-orient-parse-failures.md
  - ../howto/diagnose-brain-decision-parse-failures.md
---

# Recipe-brain verdict/decision parsing

!!! note "Migration condition"
    Current releases use these parsers. OODA consumers become legacy only after
    a release implements and selects the typed route and route verification
    proves them unreachable. They remain authoritative while the route is
    `legacy` or `shadow`; non-OODA consumers are unaffected by that cutover.
    See the planned
    [typed OODA architecture](../architecture/typed-ooda-loop.md).

> **Status — all four phases shipped.**
> Every recipe-backed brain phase reads the `recipe-runner-rs --output-format
> json` envelope, runs the shared confidence-gated escalation ladder on a
> parse-miss, and falls back loudly (never silently). The cluster is closed:
> [#2419](https://github.com/rysweet/Simard/issues/2419) (engineer-lifecycle
> JSON-envelope transport) + [#2432](https://github.com/rysweet/Simard/issues/2432)
> (escalation ladder), then the same pattern generalized to the
> **decide**/**orient** ([#2421](https://github.com/rysweet/Simard/issues/2421))
> and **merge-judge** ([#2428](https://github.com/rysweet/Simard/issues/2428) /
> [#2430](https://github.com/rysweet/Simard/issues/2430) /
> [#2435](https://github.com/rysweet/Simard/issues/2435) /
> [#2462](https://github.com/rysweet/Simard/issues/2462) /
> [#2463](https://github.com/rysweet/Simard/issues/2463)) phases, plus the
> class-level `brain_verdict_parsed_total` metric
> ([#2429](https://github.com/rysweet/Simard/issues/2429)).
>
> | Component | State | Location |
> |-----------|-------|----------|
> | engineer-lifecycle transport + ladder + metric | **shipped** ([#2419](https://github.com/rysweet/Simard/issues/2419) / [#2432](https://github.com/rysweet/Simard/issues/2432)) | `src/ooda_brain/recipe_brain.rs` |
> | decide / orient JSON transport + ladder + loud default | **shipped** ([#2421](https://github.com/rysweet/Simard/issues/2421)) | `src/ooda_brain/recipe_brain.rs` |
> | merge-judge JSON transport + ladder + fail-closed `Unclear` | **shipped** ([#2428](https://github.com/rysweet/Simard/issues/2428) / [#2430](https://github.com/rysweet/Simard/issues/2430) / [#2435](https://github.com/rysweet/Simard/issues/2435) / [#2462](https://github.com/rysweet/Simard/issues/2462) / [#2463](https://github.com/rysweet/Simard/issues/2463)) | `src/stewardship/recipe_merge_judge.rs` |
> | class-level `brain_verdict_parsed_total` metric | **shipped** ([#2429](https://github.com/rysweet/Simard/issues/2429)) | `src/ooda_brain/recipe_brain.rs` |
> | Copilot launch-log preamble stripped at the shared chokepoint + decide/orient termination-cause wiring | **shipped** ([#2496](https://github.com/rysweet/Simard/issues/2496), generalising the distill regression PR [#2500](https://github.com/rysweet/Simard/pull/2500)) | `src/recipe_output/extract.rs`, `src/ooda_brain/recipe_brain.rs` |
> | Trailing-comma recovery wired into every reasoner parse site via the shared `extract_and_parse_json` chokepoint | **shipped** ([#2658](https://github.com/rysweet/Simard/issues/2658) lineage) | `src/recipe_output/extract.rs`, `src/ooda_brain/recipe_brain.rs` |
> | Unescaped-control-character recovery (raw newline/tab/CR inside a string value) composed with trailing-comma recovery in `recover_json_view`, wired into the same chokepoint | **shipped** (#2658 lineage) | `src/recipe_output/extract.rs`, `src/ooda_brain/recipe_brain.rs` |
> | Invalid-backslash-escape recovery (a lone `\` inside a string value — a Windows path / regex / LaTeX fragment) composed into `recover_json_view` ahead of the control-char view, wired into the same chokepoint | **shipped** (#2658 lineage) | `src/recipe_output/extract.rs`, `src/ooda_brain/recipe_brain.rs` |
> | JavaScript-comment recovery (`// …` line / `/* … */` block comment outside a string value — the "JSONC" annotation shape) composed into `recover_json_view` **ahead of** the string-aware views, wired into the same chokepoint | **shipped** (#2658 lineage) | `src/recipe_output/extract.rs`, `src/ooda_brain/recipe_brain.rs` |
>
> Everything on this page describes code that exists today. A reader six months
> from now should treat this as the current design, not a migration note.

This page explains how a `recipe-runner-rs` subprocess run becomes a typed
decision or verdict, and how all four recipe-backed brain phases share one
transport, one escalation ladder, and one loud-default discipline. For the
per-parser grammar (first-word / first-float / keyword) see
[Text-parsing wire formats](./text-parsing-wire-formats.md). For the
`RecipeBrain` struct and its standalone parse functions see the
[RecipeBrain API reference](./recipe-brain-api.md). For the envelope shape
pinned against the installed `recipe-runner-rs` binary, see
[Distill recipe output capture](./distill-recipe-output-capture.md).

---

## The shared failure class (why this exists)

`recipe-runner-rs` has **two** stdout formats, selected by `--output-format`:

| Format           | Stdout content                                                                  |
|------------------|---------------------------------------------------------------------------------|
| `text` (default) | A human **summary banner** only (`Recipe: <name> … SUCCESS (NN.Ns) …`). The agent step's actual output is **not** on stdout. |
| `json`           | A structured **envelope** whose `step_results[].output` holds the agent's real output. |

Every recipe-backed brain reads the **JSON envelope** (`--output-format json`
→ `step_results[].output`), never the banner. Historically each phase read the
default text-mode banner instead, and the consequences differed per phase even
though the root cause was one wire-format bug. All four phases now extract the
real agent output before parsing:

| Phase | Adapter tag | Former text-mode failure (now fixed) | State |
|-------|-------------|--------------------------------------|-------|
| engineer-lifecycle (act) | `recipe-engineer-lifecycle-brain` | banner first word `Recipe:` → silent `continue_skipping` (~99.6% of calls) | **fixed** (#2419 / #2432) |
| decide | `recipe-decide-brain` | banner first word `Recipe:` → silent `advance_goal` every cycle | **fixed** (#2421) |
| orient | `recipe-orient-brain` | banner timing string `(0.0s)` scraped as `adjusted_urgency` → urgency corrupted to `0.0` | **fixed** (#2421) |
| merge-judge | `recipe-merge-judge` | banner contains `readiness` but no `ready`/`not_ready`/`unclear` token → no verdict, every `simard merge-pr` aborted | **fixed** (#2428 / #2430 / #2435 / #2462 / #2463) |

The fix is the same for all of them: **invoke `recipe-runner-rs` with
`--output-format json`, extract the final step's `output` from the envelope,
then run the phase parser over that real agent output** — never over the
banner. On a parse miss, climb the confidence-gated escalation ladder before
falling back, and **fall back loudly, never silently** (decide → `AdvanceGoal`,
orient → deterministic urgency floor, merge-judge → fail-closed `Unclear`).

> **What "STRUCTURED (JSON)" means here.** The structured layer is the
> `recipe-runner-rs` **transport envelope** (`--output-format json` →
> `step_results[].output`), not a change to the agent's own output grammar. The
> decide/orient/lifecycle agents still emit a first-word/first-float token and
> the merge-judge agent still emits its verdict — those agent output grammars
> are unchanged (see [Text-parsing wire formats](./text-parsing-wire-formats.md)).
> Only the **capture path** changes: from banner-scraping to envelope-extraction.

---

## Shared transport and ladder (the engineer-lifecycle reference)

Module: `src/ooda_brain/recipe_brain.rs`. The engineer-lifecycle phase is the
reference implementation of the shared machinery; the decide, orient, and
merge-judge phases reuse the very same transport, ladder backbone, and
escalation-note seam described below.

### Transport: `RecipeEnvelope` + `extract_recipe_decision_output`

```rust
/// JSON envelope returned by `recipe-runner-rs --output-format json`.
#[derive(Debug, Deserialize)]
struct RecipeEnvelope {
    success: bool,
    #[serde(default)]
    step_results: Vec<RecipeStepResult>,
}

/// A single step's result inside the envelope.
#[derive(Debug, Deserialize)]
struct RecipeStepResult {
    #[serde(default)]
    step_id: String,
    #[serde(default)]
    output: String,
}

/// Extract the decision text the agent actually produced from the
/// `--output-format json` stdout envelope (the FINAL step's `output`).
/// Shared by every recipe-backed brain phase.
pub(crate) fn extract_recipe_decision_output(stdout: &[u8], adapter_tag: &str) -> SimardResult<String>;
```

`extract_recipe_decision_output` returns the final `step_results[].output` and
surfaces a typed `SimardError::AdapterInvocationFailed` — rather than silently
returning empty text — when:

- the envelope cannot be deserialized (`failed to deserialize recipe JSON output`),
- the recipe reported `success=false` (`recipe reported success=false in JSON output`),
- no step produced output (`no step results in recipe JSON output`).

These are **genuine infrastructure failures** and stay `Err`. They are distinct
from a successful run whose agent output merely fails to parse — that is a
parse-miss, which drives the ladder rather than an `Err`. Each phase invokes the
runner with `--output-format json` in its `invoke_*_raw` helper, which
distinguishes the two on the error path (returning an `Err` plus a `cause`
label for the metric).

> `RecipeEnvelope`/`RecipeStepResult` and `extract_recipe_decision_output` are
> `pub(crate)` shared infrastructure. The merge-judge module
> (`src/stewardship/recipe_merge_judge.rs`) reuses the identical extraction via a
> `pub(crate) use` re-export from `ooda_brain`; the decide/orient phases call it
> directly. Every phase uses the same envelope decoder rather than re-deriving
> it.

### Outcome classification: `LifecycleParseOutcome`

Each parse is classified so the parse-failure rate (`outcome != parsed`) is
measurable. The same `LifecycleParseOutcome` type is the shared classification
returned by every phase's parser (`parse_action_outcome`, `parse_orient_outcome`,
`parse_merge_outcome`, `parse_lifecycle_outcome`):

| Variant | `label()` | Counts as failure? | Meaning |
|---------|-----------|--------------------|---------|
| `Parsed` | `parsed` | no | First word / verdict matched a known variant on the base attempt. |
| `DefaultEmpty` | `default_empty` | **yes** | Extracted output empty/whitespace → phase's deterministic default. |
| `DefaultMalformed` | `default_malformed` | **yes** | Output non-empty but matched no variant → phase's deterministic default. |
| `Repaired` | `repaired` | no | Real decision recovered by a **schema-repair** ladder rung. |
| `Escalated` | `escalated` | no | Real decision recovered by a **higher-effort** ladder rung. |
| `Error` | `error` | **yes** | recipe-runner spawn/exit/envelope decode failed (set on the error path, not by the pure parser). |

`LifecycleParseOutcome::is_parse_failure()` is `true` for `DefaultEmpty |
DefaultMalformed | Error`. `Repaired` / `Escalated` are real recoveries — they
do **not** count as failures, which is exactly how the ladder drops the measured
default rate.

### Confidence-gated escalation ladder (#2432)

On a base parse-miss (`DefaultEmpty` / `DefaultMalformed` — a low-confidence,
unparseable coarse judgment) the brain spends EXTRA compute only on that weak
case before reaching the deterministic default. The shared backbone is a
generic ladder parameterized over the phase's decision type `D`:

```rust
/// Generic escalation backbone shared by every recipe-backed brain phase.
/// `invoke` runs one rung (with the rung's escalation note), `parse` maps raw
/// agent output to `(D, LifecycleParseOutcome)`, and `default` supplies the
/// loud fallback when the ladder is exhausted.
pub fn run_brain_ladder<D>(
    goal_id: &str,
    base_raw: &str,
    base_outcome: LifecycleParseOutcome,
    cfg: &EscalationConfig,
    invoke: impl Fn(&LadderAttempt) -> SimardResult<String>,
    parse: impl Fn(&str) -> (D, LifecycleParseOutcome),
    default: impl Fn() -> D,
    decision_label: impl Fn(&D) -> String,
) -> (D, LifecycleParseOutcome, u32, LadderTermination);
```

The decide, orient, and merge-judge phases call `run_brain_ladder` directly.
The lifecycle-specific `run_escalation_ladder` is now a **thin wrapper** that
delegates to `run_brain_ladder`, preserving the `LifecycleInvoker` seam so the
lifecycle ladder stays unit-testable without a live `recipe-runner-rs`:

```rust
/// Seam over the raw lifecycle recipe invocation. Production wires `RecipeBrain`;
/// tests wire a scripted stub.
pub trait LifecycleInvoker {
    fn invoke_lifecycle(&self, ctx: &EngineerLifecycleCtx, attempt: &LadderAttempt)
        -> SimardResult<String>;
}

/// Thin wrapper over `run_brain_ladder` for the engineer-lifecycle decision.
pub fn run_escalation_ladder(
    invoker: &dyn LifecycleInvoker,
    ctx: &EngineerLifecycleCtx,
    base_raw: &str,
    base_outcome: LifecycleParseOutcome,
    cfg: &EscalationConfig,
) -> (EngineerLifecycleDecision, LifecycleParseOutcome, u32, LadderTermination);
```

On a base parse-miss the ladder climbs a bounded sequence of rungs
(`LadderRung`):

1. **`Base`** — the cheap attempt, byte-identical to the pre-ladder path.
2. **`SchemaRepair`** — re-prompt feeding the malformed output back, reminding
   the model of the exact accepted variant tokens.
3. **`Escalate`** — schema-repair **plus** a higher-effort, step-by-step
   reasoning instruction.

The per-rung repair note is injected through the `{{escalation_note}}` recipe
seam. A generic `build_phase_escalation_note(rung, prior_output,
repair_instruction, high_effort)` is the shared builder; each phase wraps it
with its own phrasing — `build_decide_escalation_note` and
`build_orient_escalation_note` (in `recipe_brain.rs`),
`build_merge_escalation_note` (in `recipe_merge_judge.rs`), and the unchanged
lifecycle `build_escalation_note`. **On the `Base` rung the note is empty**, so
the base attempt is byte-identical to the pre-ladder behavior. The recipe YAMLs
`ooda-decide.yaml`, `ooda-orient.yaml`, and `merge-readiness-judge.yaml` each
carry the additive `{{escalation_note}}` placeholder near the top of the prompt
(empty on the base attempt), mirroring `ooda-engineer-lifecycle.yaml`. The note
builders are content-pinned (the `escalation_note_*` tests fail CI if the
wording drifts).

The deterministic default is reached **only after the ladder is exhausted** (or
a rung's own invocation fails). Each rung logs loudly to both `tracing` and
stderr:

```
[simard] BRAIN ESCALATION goal=<id> rung=SchemaRepair attempt=2 (parse-miss recovery)
[simard] BRAIN ESCALATION goal=<id> RECOVERED decision=reclaim_and_redispatch via SchemaRepair (attempt 2)
[simard] BRAIN ESCALATION goal=<id> ladder ended (exhausted) after 3 attempts — deterministic default
```

`LadderTermination` records *which* terminal path was taken, and maps to the
metric `cause` label:

| `LadderTermination` | `cause_label()` | Meaning |
|---------------------|-----------------|---------|
| `Recovered` | `ladder_recovered` | A rung produced a parseable decision (`Repaired`/`Escalated`). |
| `Exhausted` | `ladder_exhausted` | Every configured rung tried; none parsed → deterministic default. |
| `InvokeError` | `ladder_invoke_error` | A rung's own invocation failed; ladder stopped early → deterministic default. |
| `Disabled` | `ladder_disabled` | `max_escalations == 0`; no rung tried. |

### Configuration

| Variable | Default | Hard cap | Effect |
|----------|---------|----------|--------|
| `SIMARD_BRAIN_ESCALATION_MAX_ATTEMPTS` | `2` | `3` | Escalation rungs attempted after a base parse-miss (all four phases). `0` disables the ladder (default-on-first-miss). The value is clamped to `[0, HARD_CAP]` so a misconfiguration can never create an unbounded retry loop. |

Parsed by `EscalationConfig::from_env()` / `parse_max_escalations`. No new
network surface is introduced.

---

## Metric: `brain_lifecycle_decision` (shipped)

> Closes the measurement half of [#2419](https://github.com/rysweet/Simard/issues/2419):
> emit one event per `decide_engineer_lifecycle` invocation so the lifecycle
> parse-failure rate is computable from `metrics.jsonl`.

One event is appended to `~/.simard/metrics/metrics.jsonl` per invocation
(`value = 1.0`). It is a **no-op under `cfg!(test)`** so unit tests never
corrupt the operator's real measurement file. The JSON `context` payload (built
by `build_lifecycle_metric_context`):

| Field | Values / meaning |
|-------|------------------|
| `goal_id` | The engineer-lifecycle goal id. |
| `outcome` | `parsed` \| `default_empty` \| `default_malformed` \| `error` \| `repaired` \| `escalated` (the `LifecycleParseOutcome::label()`). |
| `is_parse_failure` | `true` for `default_empty` / `default_malformed` / `error`. The numerator. |
| `first_word` | The base attempt's first token (the diagnostic record of the cheap pass — capped at 64 chars). |
| `consecutive_skip_count` | From `EngineerLifecycleCtx`. |
| `decision` | The final decision choice (`continue_skipping`, `reclaim_and_redispatch`, `deprioritize`, `open_tracking_issue`, `mark_goal_blocked`, `consider_self_update`). |
| `cause` | `ok` (parsed base) \| `ladder_recovered` \| `ladder_exhausted` \| `ladder_invoke_error` \| `ladder_disabled` (or the invoke error cause on the base-error path). |
| `attempts` | Total brain invocations spent (base + escalation rungs). |

**Lifecycle parse-failure rate** over the recorded window:

```bash
jq -rc 'select(.metric_name=="brain_lifecycle_decision")
        | .context | fromjson | "\(.outcome) is_failure=\(.is_parse_failure)"' \
  ~/.simard/metrics/metrics.jsonl \
  | sort | uniq -c
```

`parse_failure_rate = count(is_parse_failure == true) / count(*)`. Before #2419
this was ~99.6% (the banner regression); a healthy daemon trends toward `0`.

---

## Metric: `brain_verdict_parsed_total` (shipped)

> Closes [#2429](https://github.com/rysweet/Simard/issues/2429): one shared
> counter across **all** recipe-backed brain phases with a `parsed`/`defaulted`
> denominator, so each phase's parse-success rate is computable from one stream.

A `brain_verdict_parsed_total` event (`value = 1.0`) is appended to
`~/.simard/metrics/metrics.jsonl` **once per decide / orient / merge-judge
invocation**, on BOTH the parsed branch and the defaulted branch. Like the
lifecycle metric it is a **no-op under `cfg!(test)`**. The JSON `context`
payload:

| Field | Values / meaning |
|-------|------------------|
| `phase` | `decide` \| `orient` \| `merge_judge`. |
| `outcome` | `parsed` (a real decision was produced) \| `defaulted` (the loud/fail-closed fallback was taken). |
| `outcome_detail` | The `LifecycleParseOutcome::label()`: `parsed` \| `repaired` \| `escalated` \| `default_empty` \| `default_malformed` \| `error`. |
| `is_parse_failure` | `true` for `default_empty` / `default_malformed` / `error`. |
| `cause` | The `LadderTermination::cause_label()` of the run: `ladder_recovered` \| `ladder_exhausted` \| `ladder_invoke_error` \| `ladder_disabled`, or `ok` when the base attempt parsed without entering the ladder. Decide and orient now **wire the ladder's termination through to this field** (it was previously discarded as `_termination`), so a `defaulted` row attributes the default to its precise terminal path — exactly as the lifecycle metric already did. |
| `goal_id` | The phase's goal id. For merge-judge this is `pr-<N>`. |
| `attempts` | Total brain invocations spent (base + ladder rungs). |

A `defaulted` row therefore reads unambiguously: `is_parse_failure=true` with
`cause=ladder_exhausted` is a transient parse miss that survived the ladder — NOT
a model that chose to do nothing. The
[parse-failure-is-not-a-decision section](#parse-failure-is-not-a-deliberate-decision-2496)
explains why the two must stay distinct.

**Per-phase parse-success rate** over the recorded window:

```bash
jq -rc 'select(.metric_name=="brain_verdict_parsed_total")
        | .context | fromjson | "\(.phase) \(.outcome)"' \
  ~/.simard/metrics/metrics.jsonl \
  | sort | uniq -c
```

`parse_success_rate{phase} = parsed / (parsed + defaulted)` for that `phase`.

The engineer-lifecycle phase keeps its dedicated `brain_lifecycle_decision`
metric (above) unchanged; `brain_verdict_parsed_total` covers the decide,
orient, and merge-judge phases. Both are in addition to the existing
failure-only `brain_parse_failure` counter (see
[OODA brain parse-failure record](./ooda-brain-parse-failure-record.md)) and
the structured log lines.

---

## The same fix across decide, orient, and merge-judge

All three phases below ride the shared transport + ladder described above:
they invoke `recipe-runner-rs --output-format json`, extract the agent output
from the envelope via `extract_recipe_decision_output`, parse it, run
`run_brain_ladder` on a parse-miss, and only then fall back — loudly for
decide/orient, fail-closed for merge-judge. The root-cause explanation for each
phase is kept here ("previously … now …") so the contrast is unambiguous.

### Decide phase (#2421)

`recipe_brain.rs::judge_decision` invokes `invoke_decide_raw` (which passes
`--output-format json` plus the rung's `escalation_note`), extracts the agent
output from the envelope, and parses it via `parse_action_outcome(text) ->
(DecideJudgment, LifecycleParseOutcome)`. On a parse-miss it runs
`run_brain_ladder`, and only after the ladder is exhausted does it fall back
**loudly** to `DecideJudgment::AdvanceGoal` (via `default_advance_goal`). The
miss is classified `DefaultEmpty` / `DefaultMalformed`, distinct from a genuine
LLM `advance_goal` (which is `Parsed`).

Previously this call site read the default text-mode banner, whose first word is
always `Recipe:` — so it silently returned `AdvanceGoal` every cycle, ignoring
the LLM. Reading the envelope output instead means a real `advance_goal` and a
banner-induced default are no longer indistinguishable. `parse_action_from_text`
is retained as a thin decision-only wrapper over `parse_action_outcome`.

> **Launcher-preamble hardening ([#2496](https://github.com/rysweet/Simard/issues/2496)).**
> `parse_action_outcome` runs the agent output through
> [`recipe_output::strip_recipe_noise`](./text-parsing-wire-formats.md#protocol-0-shared-noise-pre-stripping-recipe_output)
> before reading the first word, so the Copilot CLI launch-log preamble
> (`ℹ NODE_OPTIONS=…`, `launching copilot binary=… version="GitHub Copilot CLI
> 1.0.66-2."`, `Run 'copilot update'…`) and its ANSI colour codes can no longer
> make the first token `ℹ`/`Run`/`1.0.66-2`. This is the fix for the production
> deadlock: with the preamble surviving, **every** active goal misparsed to
> `default_malformed`, the ladder exhausted, decide returned its NO-new-action
> default, and zero engineers spawned. `judge_decision` now also wires the
> ladder's `LadderTermination` through to the `brain_verdict_parsed_total`
> `cause` field instead of discarding it, so a parse-failure default is logged
> distinctly from a real decision (see
> [Parse failure is not a deliberate decision](#parse-failure-is-not-a-deliberate-decision-2496)).

### Orient phase (#2421)

`recipe_brain.rs::judge_orientation` invokes `invoke_orient_raw` (json +
`escalation_note`), extracts the envelope output, and parses it via
`parse_orient_outcome(text, base_urgency, failure_count) -> (OrientJudgment,
LifecycleParseOutcome)`. On a parse-miss it runs `run_brain_ladder`, then falls
back to the deterministic urgency floor (`base_urgency −
FAILURE_PENALTY_PER_CONSECUTIVE × failure_count`, i.e. `base_urgency − 0.2 ×
failure_count`, clamped to `[0.0, 1.0]`).

Previously this phase scanned the text-mode banner for the first decimal in
`[0, base_urgency]`. The banner's timing string (e.g. `(0.0s)`) was scraped as
`adjusted_urgency`, silently demoting urgency to `0.0` — worse than a benign
default. Because urgency is now read from the JSON envelope's agent output, the
banner `(0.0s)` timing string can no longer be scraped — **urgency `0.0` from a
banner can no longer happen**; the deterministic floor is the only fallback.
`parse_orient_from_text` is retained as a thin wrapper over
`parse_orient_outcome`.

> **Launcher-preamble hardening ([#2496](https://github.com/rysweet/Simard/issues/2496)).**
> `parse_orient_outcome` runs the envelope output through
> [`recipe_output::strip_recipe_noise`](./text-parsing-wire-formats.md#protocol-0-shared-noise-pre-stripping-recipe_output)
> first, so the Copilot launch-log preamble's version string
> `version="GitHub Copilot CLI 1.0.66-2."` cannot be mined as the urgency decimal
> (`1.0` / `0.66`) ahead of the model's real first float. Like decide,
> `judge_orientation` now wires the ladder's `LadderTermination` through to the
> `brain_verdict_parsed_total` `cause` field (previously discarded), so a
> deterministic-floor default reached via `ladder_exhausted` on a parse miss is
> attributable and distinct from a genuinely low urgency the model emitted.

### Merge-judge phase (#2462)

`recipe_merge_judge.rs::RecipeMergeJudge::judge` (closing #2462 / #2463 / #2428
/ #2430 / #2435) invokes `invoke_judge_raw` (json + `escalation_note`), extracts
the verdict text from the envelope, and parses it via `parse_merge_outcome(text)
-> (JudgeOutcome, LifecycleParseOutcome)`. `parse_merge_outcome` tries
`merge_judge::parse_judge_response` (structured `{"verdict":…}` JSON, which
populates `blockers`) FIRST, then falls back to the
`parse_merge_verdict_from_text` keyword scanner for prose. On a parse-miss it
runs `run_brain_ladder`, and after the ladder is exhausted it **fails CLOSED to
`Verdict::Unclear`** (via `fail_closed_unclear`).

**Fail-closed is the hard requirement.** `Unclear` is treated as a refusal by
the merge authority (see [`merge_judge.rs`](./pr-finalization-pipeline.md)). The
judge **never** flips fail-open to `Ready`, never emits SUCCESS-without-verdict,
and a recipe completing `SUCCESS` is **not** a verdict. Genuine
spawn/nonzero-exit/envelope-decode failures still propagate as `Err`
(`SimardError::AdapterInvocationFailed`). The objective deterministic gate
(`evaluate_objective_gates`) and the merge authority remain the sole deciders;
the parsed verdict is advisory input.

Previously this call site invoked `recipe-runner-rs` without `--output-format
json` and ran `parse_merge_verdict_from_text` over the raw banner. The banner
contains `readiness` but no `ready`/`not_ready`/`unclear` token, so the parser
returned `Err("no verdict keyword …")` and every `simard merge-pr` aborted at
the infrastructure level. Now `simard merge-pr` surfaces a real verdict (or a
fail-closed `Unclear` → Refused, or an explicit infra `Err`) on every run.

> Note: `parse_merge_verdict_from_text` (in `recipe_merge_judge.rs`, the keyword
> scanner) is a different function from `merge_judge::parse_judge_response` (the
> JSON-object parser). The merge judge's `parse_merge_outcome` now uses **both**:
> `parse_judge_response` for structured JSON first, then the keyword scanner as a
> prose fallback. See
> [Text-parsing wire formats §2b](./text-parsing-wire-formats.md#2b-merge-judge-recipe_merge_judgers).

---

## Parse failure is not a deliberate decision (#2496)

The deterministic default is a safety net, not a decision. A default reached
because a transient parse miss exhausted the escalation ladder is a **different
event** from the model deliberately choosing to do nothing, and the two must
never be conflated — conflating them is what let the launch-log-preamble stall
masquerade as healthy "the brain decided to take no action" behaviour while
goals with real work sat idle.

Every recipe-backed brain phase therefore keeps the distinction explicit, using
the `LifecycleParseOutcome::is_parse_failure()` classification and the
`LadderTermination` cause already produced by `run_brain_ladder`:

| Phase | Deterministic default | Parse-failure default is logged / recorded as… |
|-------|-----------------------|-----------------------------------------------|
| decide | `AdvanceGoal` (loud `default_advance_goal`) | `brain_verdict_parsed_total{phase=decide, outcome=defaulted, is_parse_failure=true, cause=ladder_exhausted\|ladder_invoke_error}` + a distinct `tracing::warn!` tagging the default as a parse-failure default (NOT a model decision). |
| orient | deterministic urgency floor | `brain_verdict_parsed_total{phase=orient, …, cause=…}` + distinct warn; the floor is attributable to the parse miss, not read as a real low urgency. |
| engineer-lifecycle | `ContinueSkipping` | `brain_lifecycle_decision{outcome=default_*, cause=ladder_exhausted\|ladder_invoke_error}` + a loud, distinct log stating the skip is a **transient parse-failure skip, re-evaluated next cycle — NOT a deliberate NO-ACTION**. |
| merge-judge | fail-closed `Unclear` (Refused) | already distinct: `Unclear` is never `Ready`; recorded with its `cause`. |

The defaults and the ladder are unchanged — they remain the rarely-needed safety
net. What changes is **visibility and attribution**: a parse-failure default is
loud, carries its `LadderTermination` cause, and is self-clearing on the next
cycle once the input is clean. With the launcher preamble now stripped at the
[shared chokepoint](./text-parsing-wire-formats.md#protocol-0-shared-noise-pre-stripping-recipe_output),
that next cycle's input *is* clean, so a goal with actionable work is no longer
parked under a NO-ACTION that was really a parse miss. The conservative
retry/skip semantics make a single transient miss harmless: the lifecycle skip is
re-evaluated, and decide re-runs the goal next cycle rather than treating one
poisoned capture as a durable decision.

For the design rationale, see
[Concept: Copilot launch-log preamble stripping § keeping a parse failure distinct from a real "no action"](../concepts/copilot-launcher-preamble-stripping.md#keeping-a-parse-failure-distinct-from-a-real-no-action).

---

## Reasoner JSON recovery at the parse chokepoint (#2658 lineage)

The shared extractor `recipe_output::extract_json_payload` strips banner / ANSI /
log noise but returns the balanced `{…}` object body **verbatim**. Four common
real-world LLM JSON defects therefore survive into the extracted payload and fail
a strict `serde_json::from_str`:

1. a **trailing comma** before a closing `}`/`]` (issue #2658),
2. an **unescaped ASCII control character** (a raw newline/tab/CR) inside a
   string value — the shape a model emits for a multi-line `content`/`rationale`
   field; `serde_json` is spec-strict and rejects it with
   `control character (\u0000-\u001F) found while parsing a string`,
3. an **invalid backslash escape** inside a string value — a lone `\` not
   followed by a JSON escape initiator (`" \ / b f n r t u`), the shape a model
   emits for a Windows path (`C:\Users`), a regular expression (`\d+`), or a
   LaTeX/Markdown fragment (`\alpha`); `serde_json` rejects it with `invalid
   escape` while parsing a string, and
4. a **JavaScript-style comment** outside a string value — a `// …` line comment
   or a `/* … */` block comment, the "JSONC" shape a model emits to annotate a
   field (`"confidence": 0.8 // high`); the JSON grammar has no comment
   production, so `serde_json` rejects the stray `/`.

Every reasoner parse site used to parse that payload strictly
(`extract_json_payload(text)?` → `serde_json::from_str(&payload).ok()?`), so one
stray comma, one literal newline, one lone backslash, OR one interleaved comment
silently dropped the model's whole structured decision and the phase fell back to
its deterministic default (a parse-failure default, per the section above — *not*
a real model decision).

All four recovery views (`strip_json_comments`; `strip_json_trailing_commas`, the
pre-existing #2658 view; `escape_json_string_control_chars`; and
`escape_json_string_invalid_escapes`) are composed by `recover_json_view` and
wired into these reasoner sites through one shared chokepoint:

```rust
pub fn extract_and_parse_json<T: serde::de::DeserializeOwned>(raw: &str) -> Option<T> {
    let payload = extract_json_payload(raw)?;
    match serde_json::from_str::<T>(&payload) {
        Ok(value) => Some(value),
        // Retry ONLY when a recovery view actually rewrote the payload (the Owned
        // arm). recover_json_view composes strip_json_comments +
        // escape_json_string_invalid_escapes + escape_json_string_control_chars +
        // strip_json_trailing_commas; each is a provable no-op (Cow::Borrowed) on
        // valid JSON, so any OTHER malformed shape returns None unchanged.
        Err(_) => match recover_json_view(&payload) {
            Cow::Owned(recovered) => serde_json::from_str::<T>(&recovered).ok(),
            Cow::Borrowed(_) => None,
        },
    }
}
```

`recover_json_view` first strips any `// …` / `/* … */` comment sitting **outside**
a string literal, then doubles any invalid backslash escape sitting **inside** a
string literal (`\d` → `\\d`), then escapes any literal control character inside a
string literal (`\b \t \n \f \r`, else `\u00XX`), then strips trailing commas. The
four views cannot interfere: the two string views only touch bytes inside string
literals, comment- and comma-stripping only touch bytes outside them.
Comment-stripping runs **first**, ahead of the two string-aware views, on purpose
— a `"` byte inside a comment (`// see "foo"`, `/* "x */`) is comment text, not a
string delimiter, so removing whole comment spans before any `in_string` scan runs
keeps every downstream string-tracking view aligned with the real string
boundaries. On a comment-free payload `strip_json_comments` borrows unchanged, so
the pre-existing three-view recovery and its ordering are preserved exactly. The
two string views are then ordered invalid-escape **before** control-char on
purpose — a lone backslash immediately followed by a raw newline (`\` + the
newline byte) is a backslash-then-control-char pair, and doubling the backslash
first lets the control-char view then escape the newline; running control-char
first would treat the raw newline as the backslash's (invalid) escape target and
leave it a raw control byte. All four are string-literal aware, so a comma,
control byte, backslash, or `//`/`/*` sequence that is legitimate string content
(a URL, a glob) is preserved, and a raw newline/tab used as JSON whitespace
*between* tokens is left untouched.

Sites routed through it in `src/ooda_brain/recipe_brain.rs`:
`parse_admission_decision`, `parse_resource_admission_decision`,
`parse_idea_dedup_decision`, `parse_idea_consolidation`, `parse_outcome_decision`,
`extract_decision_envelope` (the decide path), and `extract_orient_envelope`.

Leniency never widens beyond these four named defects: an unquoted key, an
elided array element, a missing value, or a lone `/` that is not a comment still
yields `None` (a loud parse miss + ladder escalation), exactly as before. This
improves reasoner reliability — a genuine decision that is well-formed except for
one comma, one literal newline, one lone backslash, or one annotation comment is
now honored instead of discarded — without accepting any broken JSON.

---

## Test inventory (shipped)

| Module | Coverage |
|--------|----------|
| `src/recipe_output/extract.rs` | **`issue_2496_launcher_tests`**: each Copilot launcher shape dropped by `is_copilot_launcher_line` / `is_noise_line` (the `ℹ NODE_OPTIONS=… (saved preference)` info marker, `Run 'copilot update'…`, `launching copilot binary=… version="GitHub Copilot CLI 1.0.66-2."`, leading `INFO`/`WARN` launcher lines); payload-recovery cases (a launcher+ANSI preamble wrapped around a valid action keyword, a bare urgency decimal, and a `{…}` JSON body, each surviving the clean); negative/safety cases (a `{`-leading line, an action keyword, a bare decimal, and a verdict keyword are **never** dropped); and the `Cow::Borrowed` zero-copy clean-path assertion on noise-free input. **JSON recovery views**: `strip_json_trailing_commas` (borrow on valid JSON; strip before `}`/`]`; comma-in-string and escaped-quote preserved; multibyte-safe; other-malformed unchanged), `escape_json_string_control_chars` (borrow on valid/already-escaped JSON; escape raw newline/tab/CR and generic `\u00XX` inside a string; control bytes OUTSIDE strings left as whitespace; escaped-quote and multibyte preserved), and `recover_json_view` composing both (borrow on valid JSON; owned on either defect; other-malformed stays borrowed). **`escape_json_string_invalid_escapes`** (borrow on valid JSON incl. `\\`/`\"`/`\n`/`\uXXXX`/`\/`; double a lone backslash from a regex `\d+` or a Windows path `C:\Users\model`; leave a valid escape untouched when adjacent to an invalid one; respect an escaped quote inside the string; leave a backslash OUTSIDE a string untouched; multibyte-safe), and the extended **`recover_json_view`** (recovers an invalid escape alone; composes all three defects; **orders invalid-escape before control-char** so a `\`+raw-newline pair recovers correctly). **`strip_json_comments`** (borrow on valid JSON incl. a `/`, `//`, `/*`, `*/` INSIDE a string; strip a `// …` line comment (incl. at end of input) and a `/* … */` block comment (incl. multi-line); leave a `//` inside a URL string untouched; respect an escaped quote before a `//`; a lone `/` outside a string is NOT a comment; multibyte-safe; unterminated block dropped without panic; other-malformed stays borrowed), and the further-extended **`recover_json_view`** (recovers a comment alone; strips a comment BEFORE the string-aware views so a `"` inside a comment cannot desync their string tracking; composes all four defects). **`extract_and_parse_json`** end-to-end recovery of a trailing comma and of an unescaped control character (each alone, both together, and through banner+ANSI+log noise), plus of an invalid backslash escape (alone, through banner+ANSI+log noise, and with all three defects together), plus of a line comment (alone) and a block comment (through banner+ANSI+log noise) and all four defects together, with a `//` inside a URL string preserved and a non-target-malformed body / no-object still returning `None`. |
| `src/ooda_brain/recipe_brain.rs` | `extract_recipe_decision_output` success + decode/`success=false`/empty-`step_results` error cases; `parse_lifecycle_outcome` matrix; `run_escalation_ladder` recovery / exhaustion / invoke-error / disabled paths; `LadderTermination::cause_label` distinctness; `build_escalation_note` content pins; `build_lifecycle_metric_context` shape. **`issue_2421_tests`** + **`issue_2419_family_phase_tests`**: `parse_action_outcome` / `parse_orient_outcome` classification (parsed vs `DefaultEmpty`/`DefaultMalformed`), `build_decide_escalation_note` / `build_orient_escalation_note` / `build_phase_escalation_note` content (empty on `Base`), `brain_verdict_parsed_total` context shape, and the generic `run_brain_ladder` driving an arbitrary decision type. **`issue_2496_decide_orient_launcher_tests`**: `parse_action_outcome` and `parse_orient_outcome` fed a real Copilot 1.0.66-2 banner + ANSI-coloured `INFO`/`WARN` launcher lines wrapped around a valid decision / urgency decimal, asserting the decision parses (`Parsed`, not `DefaultMalformed`) and the version string `1.0.66-2` is not mined as the urgency; the **all-goals-`DefaultMalformed` stall regression** asserting that the same noisy capture across a batch of active goals now yields parsed decisions (the deadlock no longer reproduces); and the decide/orient `cause` wiring (a parse-failure default carries `ladder_exhausted` / `ladder_invoke_error`, distinct from a genuine decision). |
| `src/memory_consolidation/distillation.rs` | **`issue_2496_distill_launcher_tests`** (built on the merged PR [#2500](https://github.com/rysweet/Simard/pull/2500) regression): the distill fact parser still recovers `{ "facts": […] }` from a launcher-preamble-wrapped capture, now via the shared `is_noise_line` chokepoint rather than a private cleaner. |
| `src/stewardship/recipe_merge_judge.rs` | `parse_merge_verdict_from_text` keyword matrix (ready / not_ready / unclear / empty / no-keyword `Err`). **`issue_2428_tests`** + **`issue_2428_production_tests`**: JSON-envelope extraction, `parse_merge_outcome` (structured `parse_judge_response` first, then keyword prose fallback), prose keyword fallback, and fail-closed `Verdict::Unclear` on an unparseable verdict. |
| `src/stewardship/merge_judge.rs` | `parse_judge_response` JSON extraction (fenced / brace-balanced / outermost), `LlmMergeJudge`, `RefusingMergeJudge`. |
| `tests/recipe_brain_verdict_assets.rs` | Asset/integration coverage of the recipe-brain verdict path. |
| `tests/gadugi/decide-orient-brain-parse.sh`, `tests/gadugi/merge-judge-verdict.sh` | Outside-in gadugi scenarios exercising the decide/orient parse path and the merge-judge verdict path end-to-end. |

---

## See also

- [How-to: Diagnose `simard merge-pr` verdict-parse failures](../howto/diagnose-merge-pr-verdict-parse-failures.md) — recognizing a real verdict, a fail-closed `unclear`, and an infra error
- [How-to: Diagnose OODA decide/orient brain parse failures](../howto/diagnose-decide-orient-parse-failures.md)
- [How-to: Diagnose OODA brain decision parse failures](../howto/diagnose-brain-decision-parse-failures.md) — the lifecycle escalation ladder (#2432)
- [Reference: RecipeBrain API](./recipe-brain-api.md)
- [Reference: Text-parsing wire formats](./text-parsing-wire-formats.md)
- [Reference: Distill recipe output capture](./distill-recipe-output-capture.md) — the envelope shape, pinned to the binary
- [Reference: PR-finalization review pipeline](./pr-finalization-pipeline.md) — the merge authority and objective gate
