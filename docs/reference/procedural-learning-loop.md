---
title: Procedural-learning loop API
description: Rust API reference and executable contract for Simard's closed episodic→procedural learning loop — the Verdict verified-signal gate and verified_outcome() mapping, the reflection_lessons module (record_failure_reflection, count_recurring_failures, maybe_distill_lesson, lesson_name and the lesson:<goal_type>:<error_class> naming convention, goal-type/error-class normalization), the LESSON_RECURRENCE_THRESHOLD / SIMARD_LESSON_RECURRENCE_THRESHOLD configuration, the brain_skill_reuse / brain_new_procedure / brain_repeat_failure metrics, the OODA cycle wiring, and the test→acceptance mapping for issues #2441 and #2458.
last_updated: 2026-06-28
owner: simard
doc_type: reference
status: implemented
related:
  - ../memory.md
  - ../concepts/procedural-learning-loop.md
  - ./ooda-procedural-memory.md
  - ./cognitive-memory-ranked-episodic-recall.md
  - ./cognitive-memory-provenance.md
  - ./automatic-distillation-scheduler.md
  - ./cognitive-memory-procedural-idempotency.md
  - ../howto/inspect-the-procedural-learning-loop.md
---

# Procedural-learning loop API

> **Status: implemented.** This page is the as-built reference for issues
> [#2441](https://github.com/rysweet/Simard/issues/2441) (skill-reuse loop) and
> [#2458](https://github.com/rysweet/Simard/issues/2458) (failure→lesson). The
> types, module, config, metrics, and tests below exist in the tree.
>
> Module: `src/memory_consolidation/reflection_lessons.rs` (gate, normalization,
> lesson naming, reflection/lesson distillation, metrics) with memory-backed
> acceptance tests in `src/memory_consolidation/tests_reflection_lessons.rs` and
> end-to-end reuse-loop guards in `src/cognitive_memory/tests_procedural_loop.rs`.
> Config field: `OodaConfig.lesson_recurrence_threshold`
> (`src/ooda_loop/types.rs`). The reuse-reinforcement seam is the existing
> `reinforce_prepared_context` (`src/memory_consolidation/mod.rs`); new-procedure
> emission is at the OODA procedure-store seam (`src/ooda_loop/cycle.rs`); metrics
> are written through the existing `record_metric` in `src/self_metrics/mod.rs`.
>
> **FU1 boundary (now satisfied).** The *mechanics* of the failure→lesson half
> were implemented and tested under #2476; their **production trigger** required
> an external failure signal. FU1 —
> [#2456](https://github.com/rysweet/Simard/issues/2456), the external-signal
> completion gate — has since landed and produces a genuine, non-self-judged
> failure signal: [`VerificationOutcome::Refuted`](#ooda-cycle-wiring), a
> *false completion* where a derivable external postcondition (PR merged, issue
> closed, self-change deployed) contradicted the done-claim. The OODA curate
> phase now wires that signal end-to-end through
> [`learn_from_verified_failures`](#ooda-cycle-wiring) →
> `record_failure_reflection` → `maybe_distill_lesson` → `brain_repeat_failure`
> (`src/ooda_loop/cycle.rs::learn_from_refuted_goals`). The loop is **never**
> gated on self-judged `ActionOutcome.success` (R10): the engineer-loop
> `VerificationReport` git-artifact probe (which only yields `verified`/
> `unverified`) is not the trigger — the gate's *refutation* is.

This page is the executable contract for the proposed procedural-learning loop.
For the rationale and the end-to-end picture see
[Closing the procedural-learning loop](../concepts/procedural-learning-loop.md);
for operations see
[Inspect the procedural-learning loop](../howto/inspect-the-procedural-learning-loop.md).

The change is designed to be **additive and backward-compatible**: the
[`CognitiveMemoryOps`](./ooda-procedural-memory.md) trait is unchanged, the
`CognitiveProcedure` shape is unchanged (no new column), and lessons are
ordinary procedures distinguished by a name prefix. Existing snapshots, IPC
payloads, and persisted nodes deserialize unchanged.

---

## The verified-signal gate

### `Verdict`

The external-signal classification that gates **all** learning. Self-judged
`ActionOutcome.success` is never used as the gate.

```rust
/// External verification verdict for an action sequence.
///
/// Sourced from a real outside signal (engineer-loop `VerificationReport`,
/// a verified subprocess exit, or a gym eval) — never from the model's own
/// `ActionOutcome.success`. `Unverified` is the fail-safe default: when no
/// external signal is available, the loop learns nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// An external check confirmed success → distil/reinforce a skill.
    VerifiedSuccess,
    /// An external check confirmed failure → reflect, maybe distil a lesson.
    /// `error_class` is the normalized failure class (e.g. `cargo_test_failure`).
    VerifiedFailure { error_class: String },
    /// No external signal available → learn nothing (fail-safe).
    Unverified,
}
```

> **Naming.** This type will live in the `reflection_lessons` module and is
> always referred to as `reflection_lessons::Verdict`. It is intentionally
> distinct from the unrelated existing `stewardship::merge_judge::Verdict`; the
> two never appear in the same scope, and module-qualified references keep them
> unambiguous. (If a bare `Verdict` import would ever collide at a call site,
> alias it — e.g. `use reflection_lessons::Verdict as LearnVerdict`.)

### `verified_outcome`

Maps an engineer-loop [`VerificationReport`](./runtime-contracts.md) to a
`Verdict`. The mapping is **conservative**: anything that is not an
unambiguous external pass/fail becomes `Unverified`.

```rust
/// Derive the learning gate from an external verification report.
///
/// - `status` of `passed` / `verified` / `success` (case-insensitive) and at
///   least one recorded check → `VerifiedSuccess`.
/// - `status` of `failed` / `error` → `VerifiedFailure { error_class }`, where
///   `error_class` is derived from the first failing check (or `summary`),
///   normalized via [`normalize_error_class`].
/// - anything else (empty status, `skipped`, `unknown`, no checks) →
///   `Unverified`.
pub fn verified_outcome(report: &VerificationReport) -> Verdict;
```

`VerificationReport` is the existing engineer-loop type
(`src/engineer_loop/types.rs`):

```rust
pub struct VerificationReport {
    pub status: String,       // "passed" | "failed" | "skipped" | …
    pub summary: String,
    pub checks: Vec<String>,  // e.g. ["cargo test: 247 passed", …]
}
```

> **Gating invariant (R10).** Every learning entry point — skill distillation,
> failure reflection, and lesson distillation — takes a `Verdict` and is a
> **no-op on `Verdict::Unverified`**. There is no code path that distils or
> reflects from `ActionOutcome.success` alone.

---

## Lesson naming

Lessons are **not** a new node type and do **not** add a column to
`CognitiveProcedure`. A lesson is an ordinary procedure whose name follows a
reserved convention:

```text
lesson:<goal_type>:<error_class>
```

`lesson_name` is the single constructor; both keys are normalized first.

```rust
/// Reserved name prefix marking a procedure as a failure-derived lesson.
pub const LESSON_NAME_PREFIX: &str = "lesson:";

/// Compose the canonical lesson procedure name for a (goal_type, error_class).
/// Both keys are normalized via [`normalize_goal_type`] / [`normalize_error_class`].
pub fn lesson_name(goal_type: &str, error_class: &str) -> String;

/// `true` if `name` is a lesson (failure-derived) procedure.
pub fn is_lesson(name: &str) -> bool; // name.starts_with(LESSON_NAME_PREFIX)
```

### Why a naming convention, not a new column

`CognitiveProcedure` is `{ node_id, name, steps, prerequisites, usage_count }`
— there is no metadata column to carry a `skill` / `lesson` tag, and adding one
would ripple through the library schema, snapshots, and IPC payloads. The
naming convention sidesteps all of that:

| Property | How the convention delivers it |
|---|---|
| **Recall** | `recall_procedure` matches name/steps with `CONTAINS`; the `goal_type` / `error_class` tokens in the name make lessons match the same objectives that produced them. |
| **Ranking** | Lessons co-rank with skills by `usage_count` in [`recall_procedures_for_objective`](./ooda-procedural-memory.md) — no separate ranking path. |
| **Classification** | `is_lesson(name)` is a pure string check; metrics and dashboards filter on the `lesson:` prefix. |
| **Back-compat** | Zero schema change; legacy procedures (no prefix) are simply non-lessons. |

### Normalization

Both keys are normalized to plain, deterministic, unit-testable strings:

```rust
/// Normalize an objective string into a goal-type key:
/// lowercased, non-alphanumeric runs collapsed to single `-`.
/// Deterministic and idempotent (e.g. "Fix CI linker OOM!" → "fix-ci-linker-oom").
pub fn normalize_goal_type(objective: &str) -> String;

/// Normalize a raw failure descriptor into an error-class key with the same
/// rules but a `_` separator (e.g. "Cargo Test FAILED" → "cargo_test_failed").
/// Deterministic and idempotent. No semantic remapping is applied — the key is
/// a pure lowercase/alphanumeric transform, so it stays stable run-to-run.
pub fn normalize_error_class(raw: &str) -> String;
```

Normalization is idempotent: `normalize_goal_type(normalize_goal_type(x)) ==
normalize_goal_type(x)` (and likewise for `normalize_error_class`), because the
chosen separators (`-` / `_`) are themselves non-alphanumeric and re-split to the
same tokens.

---

## The `reflection_lessons` module

`src/memory_consolidation/reflection_lessons.rs`. All entry points take a
`&dyn CognitiveMemoryOps` and a `Verdict`, and are no-ops on `Unverified`.

### `record_failure_reflection`

Writes a Reflexion-style reflection for a verified failure as an episodic note
tagged `reflection` / `failure`, keyed by `(goal_type, error_class)`. Returns
the stored episode id (the provenance anchor for any future lesson), or `None`
if the verdict was not a `VerifiedFailure`.

```rust
/// Generate and store a verbal failure reflection.
///
/// No-op (returns `Ok(None)`) unless `verdict` is `VerifiedFailure`.
/// The stored episode content is a short, structured reflection:
///   "Attempted <objective>. External verdict: failed (<error_class>).
///    Next time: <hint>."
/// keyed/tagged so [`count_recurring_failures`] can find it.
pub fn record_failure_reflection(
    memory: &dyn CognitiveMemoryOps,
    objective: &str,
    verdict: &Verdict,
    hint: &str,
) -> SimardResult<Option<String>>;
```

The reflection text is built by a pure helper so it can be asserted directly:

```rust
/// Build the verbal reflection body (attempted / external verdict / next time).
/// Pure and deterministic — extracted for unit testing.
pub fn reflection_text(objective: &str, error_class: &str, hint: &str) -> String;
```

### `count_recurring_failures`

Counts stored failure reflections matching a `(goal_type, error_class)` key.

```rust
/// Count `reflection`/`failure` episodes recorded for this (goal_type, error_class).
pub fn count_recurring_failures(
    memory: &dyn CognitiveMemoryOps,
    goal_type: &str,
    error_class: &str,
) -> SimardResult<u32>;
```

### `maybe_distill_lesson`

Distils a recurring failure into a `lesson:` procedure **only** when the
reflection count reaches the threshold. This is the recurrence gate that keeps
one-off failures out of procedural memory.

```rust
/// Distil a lesson from recurring reflections, gated on recurrence.
///
/// Returns `Ok(None)` when:
///   - the reflection count is `< threshold` (one-off failure), or
///   - `verdict` is not `VerifiedFailure`.
/// When the count is `>= threshold`, stores a `lesson:<goal_type>:<error_class>`
/// procedure via `store_procedure_with_provenance` (linking the source
/// reflection episodes with `PROCEDURE_DERIVES_FROM` edges, #2325) and returns
/// the lesson node id. Storing is idempotent by name (#2298): a recurring
/// (goal_type, error_class) reinforces the existing lesson's `usage_count`
/// rather than duplicating it.
pub fn maybe_distill_lesson(
    memory: &dyn CognitiveMemoryOps,
    verdict: &Verdict,
    objective: &str,
    threshold: u32,
    source_episode_ids: &[String],
) -> SimardResult<Option<String>>;
```

### `has_lesson_for`

Read helper used by the repeat-failure metric — does a lesson already exist for
this goal-type/error-class?

```rust
/// `true` if a `lesson:<goal_type>:<error_class>` procedure already exists.
/// Uses exact-name equality on recall hits (not a bare `CONTAINS` is_empty),
/// mirroring `procedure_exists` (#2298).
pub fn has_lesson_for(
    memory: &dyn CognitiveMemoryOps,
    goal_type: &str,
    error_class: &str,
) -> SimardResult<bool>;
```

---

## Configuration

### `LESSON_RECURRENCE_THRESHOLD`

```rust
/// Minimum number of `(goal_type, error_class)` reflections before a recurring
/// failure is distilled into a `lesson:` procedure. `1` would turn every
/// one-off failure into a lesson; `2` is the smallest value that excludes
/// singletons.
pub const LESSON_RECURRENCE_THRESHOLD: u32 = 2;
```

### `OodaConfig.lesson_recurrence_threshold`

The threshold is an `OodaConfig` field, populated from the environment with the
same `env_u32(key, default)` pattern as the distillation scheduler fields:

```rust
pub struct OodaConfig {
    // …
    /// Recurrence threshold before a repeated failure becomes a lesson
    /// (`SIMARD_LESSON_RECURRENCE_THRESHOLD`, default 2).
    #[serde(default = "default_lesson_recurrence_threshold")]
    pub lesson_recurrence_threshold: u32,
    // …
}

fn default_lesson_recurrence_threshold() -> u32 {
    LESSON_RECURRENCE_THRESHOLD // 2
}
```

| Setting | Env var | Default | Effect |
|---|---|---|---|
| Lesson recurrence threshold | `SIMARD_LESSON_RECURRENCE_THRESHOLD` | `2` | Reflections per `(goal_type, error_class)` before a lesson is distilled. `1` ⇒ lessons from one-off failures (not recommended); higher ⇒ require more repeats. |

No configuration will be required — the default value (`2`) is the intended
production default. The verified-signal gate has no toggle: it is a load-bearing
invariant, not a tunable.

---

## Metrics

Three metrics, all written via
[`record_metric(name, value, context)`](./brain-introspection-api.md) to
`~/.simard/metrics/metrics.jsonl`. `value` is always `1.0` (a count event);
`context` is a compact JSON object.

| Metric name | Fires when | `context` fields |
|---|---|---|
| `brain_skill_reuse` | a recalled procedure is **applied** (surfaced into the cycle prompt) and reinforced | `{ "procedure_id", "kind" }` where `kind` is `skill` or `lesson` |
| `brain_new_procedure` | a new skill or lesson procedure is stored for the first time (not a reinforcing re-store) | `{ "procedure_id", "name", "kind" }` |
| `brain_repeat_failure` | a `VerifiedFailure` occurs on a goal-type that **already** has a matching `lesson:` procedure | `{ "goal_type", "error_class", "lesson_id" }` |

`brain_repeat_failure` is the loop's self-regression signal: a lesson exists but
the failure repeated anyway, so the lesson did not take. See
[Inspect the procedural-learning loop](../howto/inspect-the-procedural-learning-loop.md#read-the-loop-metrics).

Example records (`metrics.jsonl`):

```json
{"timestamp":"2026-06-28T05:30:00Z","metric_name":"brain_skill_reuse","value":1.0,"context":"{\"procedure_id\":\"proc_4f2a\",\"kind\":\"lesson\"}"}
{"timestamp":"2026-06-28T05:31:12Z","metric_name":"brain_new_procedure","value":1.0,"context":"{\"procedure_id\":\"proc_91c\",\"name\":\"lesson:fix-ci-linker-oom:cargo_test_failure\",\"kind\":\"lesson\"}"}
{"timestamp":"2026-06-28T05:42:55Z","metric_name":"brain_repeat_failure","value":1.0,"context":"{\"goal_type\":\"fix-ci-linker-oom\",\"error_class\":\"cargo_test_failure\",\"lesson_id\":\"proc_91c\"}"}
```

---

## OODA cycle wiring

The loop has four production seams, all wired on honest external signals.

**Wired now (honest signals):**

1. **Apply → reinforce → `brain_skill_reuse`.** When recalled procedures are
   surfaced into a cycle's prompt, `reinforce_prepared_context`
   (`src/memory_consolidation/mod.rs`, invoked from `advance.rs`) bumps each
   recalled procedure's `usage_count` and emits `brain_skill_reuse` via
   [`record_skill_reuse`]. This is the measurable close of the recall→rerank
   loop — applying a recalled procedure is, definitionally, reuse.
2. **New procedure → `brain_new_procedure`.** When the OODA consolidation seam
   (`src/ooda_loop/cycle.rs`) stores a *new* procedure (not a reinforcing
   re-store), it emits `brain_new_procedure` via [`record_new_procedure`].
3. **Refuted completion → reflection → lesson → `brain_repeat_failure`.** FU1
   (#2456) landed the external-signal completion gate. In the OODA **curate**
   phase, `archive_completed_evidence_aware` classifies each blocked completion;
   a goal whose done-claim a *derivable external postcondition refuted*
   (`VerificationOutcome::Refuted` — PR not merged, issue still open, self-change
   not deployed) is a genuine, non-self-judged failure. `learn_from_refuted_goals`
   (`src/ooda_loop/cycle.rs`) maps each refuted goal to a
   [`VerifiedFailureObservation`] (`{objective, error_class}` where `error_class`
   comes from [`error_class_from_missing`]) and hands
   the batch to [`learn_from_verified_failures`], which records a reflection,
   distils a recurrence-gated lesson, and emits `brain_repeat_failure` when a
   failure recurs on a goal-type that already carries a lesson. `Unverified` /
   `Error` outcomes (nothing to check, or a query failed) are skipped — they are
   not failures (R10). **Per-occurrence dedup:** each observation carries the
   goal id as an `occurrence_key`, so a goal that stays blocked across many
   cycles (e.g. a normal in-flight PR not yet merged) reflects exactly **once** —
   bounding episodic growth and keeping recurrence honest (distinct
   false-completions, not cycle count). Recurrence therefore accrues across
   *distinct* goals/attempts of the same type, never from one stuck goal.

```rust
// src/ooda_loop/cycle.rs — curate phase (simplified).
let (archived, blocked) = goal_curation::archive_completed_evidence_aware(board, source);
// Only goals an external postcondition *refuted* drive learning (never
// self-judged ActionOutcome.success). Recurring refutations distil a lesson.
learn_from_refuted_goals(&blocked, &*clients.memory, config.lesson_recurrence_threshold);

// learn_from_refuted_goals filters to VerificationOutcome::Refuted, derives the
// error_class from the missing evidence, then for each observation:
//   record_failure_reflection → maybe_distill_lesson → (repeat) record_repeat_failure
```

> **Skill-distillation gate (seam 2) and self-report.** The existing OODA
> consolidation stores procedures on a cycle's self-assessed `outcome.success`
> (pre-existing #2281 behaviour). `ActionOutcome` carries no external verdict at
> that seam, so the verified-signal skill gate
> ([`should_distill_skill`] / [`verified_outcome`]) ships ready for an
> engineer-loop `VerificationReport` seam where a real report exists. R10 is
> preserved: the **failure** path (seam 3) is gated exclusively on the gate's
> external refutation, never on self-judged success.

---

## Backward compatibility

| Aspect | Before | After |
|---|---|---|
| `CognitiveMemoryOps` trait | unchanged | unchanged (no new methods) |
| `CognitiveProcedure` shape | `{node_id,name,steps,prerequisites,usage_count}` | unchanged (lessons are name-prefixed) |
| Schema / snapshots / IPC | — | unchanged; no DDL |
| Skill distillation gate | self-judged `outcome.success` | gate **implemented** ([`verified_outcome`]/[`should_distill_skill`]); production flip awaits an engineer-loop report seam — R10 preserved |
| Failure handling | none | reflection + recurrence-gated lesson, **wired live** on the #2456 `Refuted` signal (`learn_from_refuted_goals`) |
| Metrics | — | `brain_skill_reuse` (wired), `brain_new_procedure` (wired), `brain_repeat_failure` (**wired** on refuted completions) |

---

## Test → acceptance mapping

The contract is enforced by the tests below — pure gate/normalization/metric
contracts in `reflection_lessons`'s inline `#[cfg(test)] mod tests`, memory-backed
behaviour in `src/memory_consolidation/tests_reflection_lessons.rs`, the live
curate-phase wiring in `src/ooda_loop/cycle.rs::tests_refuted_lessons`, and the
end-to-end reuse-loop guards in `src/cognitive_memory/tests_procedural_loop.rs`.

| # | Acceptance criterion | Test | Status |
|---|---|---|---|
| AC-1 (#2441) | Usage-ranked recall: a higher-`usage_count` matching procedure outranks a lower one | `recall_procedure_ranks_more_used_procedure_first` | ✅ |
| AC-2 (#2441) | Distill→recall→apply reinforces `usage_count` (0→1); applying records `brain_skill_reuse` | `reinforce_prepared_context_bumps_surfaced_fact_and_procedure_usage` (reinforce); `reuse_feeds_back_into_recall_order_closes_the_loop` (loop); `metric_contexts_have_expected_shapes` (metric shape) | ✅ |
| AC-3 (#2441) | A later similar objective recalls and applies the distilled skill | `reuse_feeds_back_into_recall_order_closes_the_loop` | ✅ |
| AC-4 (#2458) | A `VerifiedFailure` produces a `reflection:failure` episode | `verified_failure_writes_reflection` | ✅ |
| AC-5 (#2458) | A recurring `(goal_type,error_class)` (≥ threshold) becomes a retrievable `lesson:` procedure | `recurring_failure_becomes_recallable_lesson` | ✅ |
| AC-6 (#2458) | A one-off failure (count = 1) does **not** become a lesson | `one_off_failure_is_not_a_lesson` | ✅ |
| AC-7 (#2441/#2458) | A subsequent attempt on the failed goal-type surfaces the lesson | `recurring_failure_becomes_recallable_lesson` (recall assertion) | ✅ |
| AC-8 (R10) | `Verdict::Unverified` distils/reflects nothing | `unverified_and_success_write_no_reflection`, `unverified_distills_no_lesson_even_with_reflections` | ✅ |
| AC-9 (R9) | `brain_repeat_failure` records when a `VerifiedFailure` recurs on a goal-type with an existing lesson | `learn_from_verified_failures_counts_repeat_after_lesson`, `learn_from_verified_failures_distill_and_repeat_in_one_pass` | ✅ |
| AC-10 | `verified_outcome` maps pass/fail/unknown reports to the correct `Verdict` | `verified_outcome_mapping` | ✅ |
| AC-11 | `normalize_goal_type` / `normalize_error_class` are deterministic and idempotent | `normalization_is_idempotent` | ✅ |
| AC-12 (#2458) | The #2456 `Refuted` signal drives the live curate-phase loop (reflection + recallable lesson); `Unverified`/`Error` are skipped | `tests_refuted_lessons::*` (cycle), `error_class_from_missing_*` (gate) | ✅ |

All ACs are green under `cargo test`. The failure→lesson production trigger is
now live on the #2456 `Refuted` signal via `learn_from_refuted_goals` in the
OODA curate phase (see [OODA cycle wiring](#ooda-cycle-wiring)). No
snapshot/architecture-snapshot docs are added, and no live redeploy is part of
this change.
