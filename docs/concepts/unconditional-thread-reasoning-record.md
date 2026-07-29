---
title: "Unconditional thread-reasoning record — no early-exit escape past the ACT step"
description: >
  Why every reflective cognitive-thread recipe now writes its typed
  ThreadReasoningRecord on every path — including the "nothing durable to keep"
  path — so the fail-CLOSED R1 rail reader (read_verified_thread_reasoning) can
  never trip on a spurious absent record with "no record at expected path: No such
  file or directory (os error 2)". Covers the OODA reflection-step incident, the
  root cause (a recipe-level early-exit that skipped the REQUIRED record step), the
  de-contradiction applied to the seven reflective recipes that carried the escape
  (two others already recorded unconditionally, and the invariant now guards all
  nine), why the R1 fail-CLOSED gate is preserved (not weakened to a default), and
  the optional additive ENOENT self-diagnosis signature. Closes #4986.
last_updated: 2026-07-29
review_schedule: as-needed
owner: simard
doc_type: explanation
status: implemented
related:
  - ../reference/unconditional-thread-reasoning-record-contract.md
  - ../reference/simard-cognition-record-thread-reasoning-cli.md
  - ../reference/cognitive-threads-catalog.md
  - ../reference/cognitive-thread-observability.md
  - ../reference/creative-ideas-durable-read-after-write.md
  - ../reference/terminal-failure-diagnosis-api.md
  - ./self-diagnose-on-step-error.md
  - ./overseer-root-cause-why.md
  - ../../prompt_assets/simard/recipes/reflect-postmortem.yaml
  - ../../src/cognitive_threads/recipe_rail.rs
  - ../../src/ooda_brain/thread_reasoning_record.rs
  - ../../src/cognitive_threads/tests_rework_contract.rs
---

# Unconditional thread-reasoning record — no early-exit escape past the ACT step

> **Status: implemented.** The fix reframes the "nothing durable to keep" branch
> in the **seven** reflective recipes that carried an early-exit escape under
> [`prompt_assets/simard/recipes/`](https://github.com/rysweet/Simard/tree/main/prompt_assets/simard/recipes)
> so the REQUIRED
> [`simard cognition record-thread-reasoning`](../reference/simard-cognition-record-thread-reasoning-cli.md)
> ACT step runs unconditionally. Two recipes (`salience-appraise`,
> `narrative-identity`) already recorded unconditionally and need no change; the
> strengthened contract test now **guards all nine**. The fail-CLOSED R1 gate in
> [`read_verified_thread_reasoning`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/thread_reasoning_record.rs)
> is preserved verbatim. The strengthened contract test lives in
> [`src/cognitive_threads/tests_rework_contract.rs`](https://github.com/rysweet/Simard/blob/main/src/cognitive_threads/tests_rework_contract.rs).
> Closes [#4986](https://github.com/rysweet/Simard/issues/4986).

This is the narrative for a single self-diagnosed OODA reflection-step failure and
the recipe-level fix that resolved it. For the exact wire contract — which recipes,
the unconditional-record invariant, the strengthened grep gate, and the optional
ENOENT diagnosis signature — read the
[Unconditional thread-reasoning record contract](../reference/unconditional-thread-reasoning-record-contract.md).

## The incident

The Overseer's self-diagnosis loop (see
[Self-diagnose on step error](./self-diagnose-on-step-error.md)) caught a failed
OODA step and surfaced it under the reflection cognitive-thread label:

```
cognitive-thread: reflection: reflection: FAILED — R1 no record at expected path:
No such file or directory (os error 2)
```

The step exited with the daemon's canonical fail-CLOSED line. The Overseer asked
**WHY** (guideline **G3: agentic over brittle heuristics**) rather than logging and
moving on — exactly the [operator root-cause principle](./overseer-root-cause-why.md).

## Reading "R1" correctly

`R1` is **not** "reflection round 1." Across every `read_verified_thread_reasoning`
reader, `R1` is **row 1 of the fail-CLOSED read matrix: the record file is absent
or unreadable (`ENOENT`)**. The emitted tail — `No such file or directory (os error
2)` — is the OS reporting a missing file. The reflection thread label in the log is
only the daemon printer attributing the downstream read failure to the thread that
ran the recipe; the reflection thread itself performs no durable read or write.

So the true question is narrow: **why was the record file absent at the path the
rail reader resolved?**

## Root cause: an early-exit that skipped the REQUIRED ACT step

`run_reflective_thread`
([`src/cognitive_threads/recipe_rail.rs`](https://github.com/rysweet/Simard/blob/main/src/cognitive_threads/recipe_rail.rs))
follows a strict read-after-write contract:

1. Derive the per-thread record path
   (`state_root/cognitive_threads/reasoning/<thread>.json`) and **delete any leftover
   record before spawning** (anti-replay).
2. Capture `invoke_start`, then invoke the recipe with `-c record_path=<abs>`.
3. On exit `0`, the **only** source of truth is the typed record the recipe wrote
   via its ACT step. Read it **fail-CLOSED**: an absent record is `R1 → Err`, logged
   as `FAILED — R1 …`. There is no stdout fallback and no `unwrap_or` default.

The defect was in the **recipe prompt**, not the rail. `reflect-postmortem.yaml`
(and six of its siblings) contained a contradiction:

```text
1. Decide whether the outcome is worth a durable post-mortem. If it is pure
   noise … write nothing and finish successfully.   ← early-exit escape
…
## Record your reasoning (REQUIRED final ACT step)                ← REQUIRED
   … do not skip it.
```

When the reflection agent hit the "pure noise" path — a trivial success or an exact
repeat of a prior post-mortem — it took the escape literally: it wrote **nothing**
(no memory fact **and** no reasoning record) and exited `0`. The recipe "ran," so
the rail then read the record fail-CLOSED, found the file absent, and correctly
tripped `R1`. The fail-CLOSED gate did exactly its job; the **record was
spuriously absent** because the recipe let the agent walk past the REQUIRED write.

This is the same class of latent contradiction as the
[creative-ideas durable read-after-write](../reference/creative-ideas-durable-read-after-write.md)
defect: a reader that requires a record, paired with a producer path that can skip
writing it.

## The fix: the record step is unconditional

The remedy is the smallest change that makes the step succeed **without** weakening
the gate: **decouple the memory-write decision from the reasoning-record write.**

- The "nothing durable to keep" decision now governs only whether `simard memory
  remember` / `remember-procedure` are called. It **no longer** authorizes exiting
  early.
- The `simard cognition record-thread-reasoning` ACT step is the **single terminal
  ACT on every path**. Even a "pure noise" reflection records its one-to-three
  sentence conclusion (e.g. *"Nothing durable to keep this cycle: the goal was a
  trivial success already covered by prior post-mortems."*). That reasoning is
  exactly what the operator log wants, and it guarantees the record file exists at
  the resolved path.

The identical de-contradiction was applied to the **seven** reflective recipes
that actually carried the early-exit escape — `reflect-postmortem`,
`metacognition-appraise`, `prospect-foresight`, `operator-model`,
`consolidate-sleep`, `analogy-map`, and `values-deliberate`. Two recipes,
`salience-appraise` (its `... signal) and finish.` branch already falls through to
the record step) and `narrative-identity` (no escape phrase at all), were
**already compliant** and needed no edit. The strengthened contract test now
guards **all nine**, so the same OODA thread failure cannot resurface on any of
them. See the
[contract reference](../reference/unconditional-thread-reasoning-record-contract.md#the-nine-reflective-recipes)
for the full list and pre-fix state.

## Why not weaken R1 to a default?

Making the reader treat an absent record as an empty/OK default would "fix" the
symptom by **deleting the invariant**. The R1-absent-is-`Err` behaviour is a
correctness control asserted across
[`src/cognitive_threads/tests_thread_reasoning_record.rs`](https://github.com/rysweet/Simard/blob/main/src/cognitive_threads/tests_thread_reasoning_record.rs):
a recipe that "ran" but wrote no valid record is a **failure**, not a silent
success. Weakening it would let a genuinely broken recipe pass unnoticed — the exact
boolean-`ok` collapse this subsystem was built to eliminate. The fix therefore
guarantees the record is **present**, and leaves fail-CLOSED reading untouched.

## Self-diagnosis for a recurrence (optional, additive)

`classify_cause`
([`src/overseer/diagnosis.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/diagnosis.rs))
has no "no such file" signature, so a bare `os error 2` classifies as
`FailureCause::Unknown` — correct, and code-fixable. An **additive** ENOENT /
absent-reasoning-record signature can be recognised so a future R1 recurrence
self-diagnoses to a clear cause and remedy, **without** changing the shape or
existing behaviour of `classify_terminal_failure`. Because every `FailureCause`
variant maps to a stable label, the additive variant also gets a matching
`FailureCause::as_str` arm (e.g. `"missing-reasoning-record"`). This is optional
hardening; the primary fix is the recipe change above.

## Boundary and non-goals

- **Rail unchanged.** `run_reflective_thread`, the anti-replay pre-truncate, the
  `invoke_start`/`mtime` freshness (R7), and the `FAILED — R{n}` log format are all
  untouched. Only the recipe prompts changed.
- **Reader unchanged.** `read_verified_thread_reasoning` and its R1–R7 matrix keep
  fail-CLOSED semantics; no record is ever defaulted.
- **Writer unchanged.** `record-thread-reasoning` still `create_dir_all`s the parent,
  hardens the path (absolute, rejects `..`), and caps the payload at 64 KiB.
- **No `reflection.rs` edit.** The thread's health-from-exit-status logic is
  correct and out of scope.
- **No Bridge naming, no stray `print!`/`println!`,** additive and CI-green — the
  remedy honours the [self-diagnose](./self-diagnose-on-step-error.md) output rules.
