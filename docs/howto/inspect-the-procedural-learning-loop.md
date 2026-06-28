---
title: Inspect the procedural-learning loop
description: Operator guide for Simard's closed episodic→procedural learning loop — confirming skills are recalled and reused, watching verified failures become Reflexion-style reflections and recurring failures become lessons, reading the brain_skill_reuse / brain_new_procedure / brain_repeat_failure metrics, listing skill vs lesson procedures, tuning the lesson recurrence threshold, and verifying that the verified-signal gate suppresses learning when no external signal exists.
last_updated: 2026-06-28
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/procedural-learning-loop.md
  - ../reference/procedural-learning-loop.md
  - ../reference/ooda-procedural-memory.md
  - ../reference/simard-memory-cli.md
  - ../reference/automatic-distillation-scheduler.md
  - ../howto/configure-episode-hygiene-and-promotion.md
  - ../memory.md
---

# Inspect the procedural-learning loop

> **Status: implemented**, with one boundary. The skill-reuse half is live:
> `brain_skill_reuse` (apply-time reuse) and `brain_new_procedure` (new
> skill/lesson stored) are written to `~/.simard/metrics/metrics.jsonl`, and the
> `SIMARD_LESSON_RECURRENCE_THRESHOLD` knob is read by `OodaConfig`. The
> failure→lesson surface (stored reflections, recurring `lesson:` procedures, and
> `brain_repeat_failure`) is implemented and tested, but **does not fire in
> production until an external failure signal (FU1) exists** — today no
> engineer-loop verdict reports `failed`, and the gate never fires on self-judged
> success. So you will see `brain_skill_reuse` / `brain_new_procedure` events
> now, but not reflections / lessons / `brain_repeat_failure` until FU1 lands.
> Applies to issues
> [#2441](https://github.com/rysweet/Simard/issues/2441) and
> [#2458](https://github.com/rysweet/Simard/issues/2458). For the design see
> [Closing the procedural-learning loop](../concepts/procedural-learning-loop.md).

Once implemented, Simard will turn recurring **verified** successes into
reusable *skills* and recurring **verified** failures into *lessons*, then
recall and apply both to condition future cycles. This guide shows how to watch
that loop run, read its metrics, list what it has learned, and tune the one knob
it will expose.

The loop will need no configuration to work — the defaults are intended to be
production-ready. Tune only when you have a reason.

## When to use this

- You want to confirm distilled procedures are actually being **recalled and
  reused**, not just stored.
- You want to see a repeated failure become a durable lesson.
- You are auditing whether a goal-type keeps failing *despite* a lesson
  (`brain_repeat_failure`).
- You want to tune how many repeats it takes before a failure becomes a lesson.

## Watch skills get reused (#2441)

Each OODA cycle logs which procedures it recalled for the current objective.
Tail the daemon journal and look for the structured recall line:

```text
recalled procedures for objective  procedure_count=3 procedure_names="ooda:fix-ci:cargo … | lesson:fix-ci-linker-oom:cargo_test_failure | ooda:triage:pr"
```

When a recalled procedure is **applied** (surfaced into the cycle's prompt),
its `usage_count` is reinforced and a reuse metric is recorded. Confirm reuse
is happening:

```bash
grep brain_skill_reuse ~/.simard/metrics/metrics.jsonl | tail
```

```json
{"metric_name":"brain_skill_reuse","value":1.0,"context":"{\"procedure_id\":\"proc_4f2a\",\"kind\":\"skill\"}"}
```

A healthy loop shows `brain_skill_reuse` events accumulating and the
`usage_count` of useful procedures climbing over time. If you see procedures
stored but **no** `brain_skill_reuse` events, the loop is write-only — recall
or apply is not firing; check that preparation is recalling for the objective
(the `recalled procedures for objective` line above).

## Watch failures become lessons (#2458)

On a **verified** failure, Simard writes a short reflection (what was
attempted, the external verdict, what to try next) as a `reflection`/`failure`
episode. A *one-off* failure stops there. When the same
`(goal_type, error_class)` recurs at least the recurrence threshold (default
**2**) times, the reflection is distilled into a `lesson:` procedure.

Watch the failure path in the journal:

```text
[simard] reflection-lessons: recorded failure reflection (goal_type=fix-ci-linker-oom error_class=cargo_test_failure)
[simard] reflection-lessons: recurrence 2 >= threshold 2 → distilled lesson 'lesson:fix-ci-linker-oom:cargo_test_failure'
```

The lesson is now an ordinary procedure, so it appears in the next cycle's
recall for matching objectives and co-ranks with skills by `usage_count`.

## List skills vs lessons

Lessons are procedures whose name starts with `lesson:`; skills start with
`ooda:`. Use the existing read-only memory CLI to dump procedural memory and
filter by prefix (see [Memory introspection CLI](../reference/simard-memory-cli.md)):

```bash
# sample procedural-memory rows (most-used first)
simard memory dump --type=procedural --limit=50

# lessons only
simard memory dump --type=procedural --limit=50 | grep 'lesson:'

# skills only
simard memory dump --type=procedural --limit=50 | grep 'ooda:'
```

Each lesson's name encodes the `goal_type` and `error_class` it guards
against: `lesson:<goal_type>:<error_class>`.

## Read the loop metrics

All three loop metrics are appended to `~/.simard/metrics/metrics.jsonl` and
are also surfaced through brain introspection
([Brain Introspection API](../reference/brain-introspection-api.md)).

| Metric | What a rising count means |
|---|---|
| `brain_skill_reuse` | recalled procedures are being applied — the loop is closed and working |
| `brain_new_procedure` | new skills/lessons are being learned |
| `brain_repeat_failure` | **regression**: a goal-type failed again *despite* an existing lesson |

Quick health check:

```bash
for m in brain_skill_reuse brain_new_procedure brain_repeat_failure; do
  printf '%-22s %s\n' "$m" "$(grep -c "\"metric_name\":\"$m\"" ~/.simard/metrics/metrics.jsonl)"
done
```

A persistently climbing `brain_repeat_failure` for one `goal_type` means the
lesson is not changing behaviour — inspect that lesson's `steps` and the
reflections behind it (its `PROCEDURE_DERIVES_FROM` provenance, see
[Cognitive-memory provenance](../reference/cognitive-memory-provenance.md)).

## Tune the recurrence threshold

The only knob is how many repeats it takes before a failure becomes a lesson.
Default is `2` (one-off failures are never distilled).

```bash
# require three repeats before distilling a lesson (less eager)
export SIMARD_LESSON_RECURRENCE_THRESHOLD=3

# distil a lesson from the first failure (NOT recommended — turns noise into lessons)
export SIMARD_LESSON_RECURRENCE_THRESHOLD=1
```

The value is read into `OodaConfig.lesson_recurrence_threshold` at daemon
start. Leave it at `2` unless lessons are too noisy (raise it) — there is no
supported value that disables reflections; only lesson *distillation* is gated.

## Verify the verified-signal gate

The loop only learns from an **external** verdict. When no verification signal
is available, the verdict is `Unverified` and Simard learns nothing — this is
intentional fail-safe behaviour, not a bug.

To confirm the gate is holding, run a cycle whose action produces no external
verification and check that **no** new procedure and **no** reflection were
written for it:

```bash
# count procedures + reflection episodes before and after a self-assessed-only cycle
simard memory stats     # note procedural + episodic counts
# … run one cycle with no external verification …
simard memory stats     # counts for procedural/lesson must be unchanged
```

If you see procedures or reflections appear from a cycle that had no external
check, the gate is leaking — that is a defect (the gate must be a no-op on
`Verdict::Unverified`). See the gating invariant in the
[API reference](../reference/procedural-learning-loop.md#the-verified-signal-gate).

## Troubleshooting

| Symptom | Likely cause | What to check |
|---|---|---|
| Procedures stored but `brain_skill_reuse` never fires | recall/apply not wired to objective | `recalled procedures for objective` journal line; objective tokens vs procedure names |
| Repeated failures never become lessons | threshold too high, or `error_class` not stable across runs | `SIMARD_LESSON_RECURRENCE_THRESHOLD`; that `normalize_error_class` yields the same key each time |
| `brain_repeat_failure` climbing for one goal-type | the lesson is not changing behaviour | the lesson's `steps`; its source reflections via provenance |
| Nothing learned at all | every verdict is `Unverified` | whether the action path produces a `VerificationReport` with a real `status` |

## See also

- [Closing the procedural-learning loop](../concepts/procedural-learning-loop.md) — the design.
- [Procedural-learning loop API](../reference/procedural-learning-loop.md) — the contract.
- [OODA procedural memory](../reference/ooda-procedural-memory.md) — the store/recall substrate.
- [Configure episode hygiene and promotion](../howto/configure-episode-hygiene-and-promotion.md) — the related distillation scheduler.
