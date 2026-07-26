---
title: Configure the reflective cognitive threads
description: >
  How to enable, disable, and observe the nine reflective cognitive threads (reflection,
  metacognition, salience, narrative, consolidation, prospection, values-deliberation,
  analogy, operator-model). Covers the double-default-OFF gate model, the per-thread
  environment switches, verifying that a thread's effects land through the memory tools,
  and confirming a dormant deployment is a zero-behavior change.
last_updated: 2026-07-26
owner: simard
doc_type: howto
related:
  - ../architecture/reflective-cognitive-threads.md
  - ../reference/simard-cognition-salience-signal-cli.md
  - ../reference/simard-memory-remember-cli.md
  - ../reference/cognitive-thread-scheduling.md
  - ./configure-cognitive-thread-scheduling.md
  - ./add-a-new-cognitive-thread.md
---

# Configure the reflective cognitive threads

The nine reflective cognitive threads run inside the `Mind` scheduler on their
own cadence. Each performs its side effects by calling `simard` tools **from
inside its recipe** — see
[Reflective cognitive threads act via tools](../architecture/reflective-cognitive-threads.md).
This guide shows how to turn them on, off, and verify them.

## Prerequisites

- A running Simard daemon (`simard daemon`) — the threads live in the daemon
  process and run after each authoritative OODA cycle.
- The daemon's memory IPC socket is live (the reflective recipes reach it via the
  `SIMARD_MEMORY_SOCKET` the thread invoker exports into the recipe subprocess).

## The double-default-OFF gate model

Every reflective thread is dormant until **two** gates are truthy:

1. **Master gate** — `SIMARD_COGNITIVE_THREADS_ENABLED` must be truthy for the
   `Mind` to host any reflective thread at all.
2. **Per-thread gate** — `SIMARD_THREAD_<NAME>_ENABLED` must be truthy for that
   specific thread to run.

Both default to OFF. If either is unset/falsey, the thread never fires. This is
why merging the feature dormant is a **zero-behavior change**.

Truthy values follow the standard Simard convention (`1`, `true`, `yes`, `on`,
case-insensitive); anything else — including unset — is OFF.

### Per-thread gate names

| Thread | Per-thread gate | Recipe triggered |
| --- | --- | --- |
| reflection | `SIMARD_THREAD_REFLECTION_ENABLED` | `reflect-postmortem` |
| metacognition | `SIMARD_THREAD_METACOGNITION_ENABLED` | `metacognition-appraise` |
| salience | `SIMARD_THREAD_SALIENCE_ENABLED` | `salience-appraise` |
| narrative | `SIMARD_THREAD_NARRATIVE_ENABLED` | `narrative-identity` |
| consolidation | `SIMARD_THREAD_CONSOLIDATION_ENABLED` | `consolidate-sleep` |
| prospection | `SIMARD_THREAD_PROSPECTION_ENABLED` | `prospect-foresight` |
| values-deliberation | `SIMARD_THREAD_VALUES_ENABLED` | `values-deliberate` |
| analogy | `SIMARD_THREAD_ANALOGY_ENABLED` | `analogy-map` |
| operator-model | `SIMARD_THREAD_OPERATOR_MODEL_ENABLED` | `operator-model` |

The deterministic `interoception` thread has its own gate
(`SIMARD_THREAD_INTEROCEPTION_ENABLED`) but triggers **no recipe** — it does pure
Rust sensing.

## Enable one thread

Enable the master gate plus the specific per-thread gate, then (re)start the
daemon so it picks up the environment:

```bash
export SIMARD_COGNITIVE_THREADS_ENABLED=1
export SIMARD_THREAD_REFLECTION_ENABLED=1
simard daemon
```

Only `reflection` will run; the other eight stay dormant because their per-thread
gates are still OFF.

## Enable several threads

```bash
export SIMARD_COGNITIVE_THREADS_ENABLED=1
export SIMARD_THREAD_REFLECTION_ENABLED=1
export SIMARD_THREAD_METACOGNITION_ENABLED=1
export SIMARD_THREAD_SALIENCE_ENABLED=1
simard daemon
```

## Disable a thread (or all of them)

- Turn off a single thread: unset (or set falsey) its `SIMARD_THREAD_<NAME>_ENABLED`.
- Turn off **all** reflective threads at once: unset (or set falsey) the master
  gate `SIMARD_COGNITIVE_THREADS_ENABLED`. The per-thread gates then no longer
  matter — nothing runs.

```bash
unset SIMARD_COGNITIVE_THREADS_ENABLED   # kills all reflective threads
```

## Verify a thread's effects landed

Because there is **no JSON envelope** and **no output file**, you verify a thread
by observing its **tool calls' effects**, not by scraping recipe output.

- **Facts / procedures** — check the store the daemon reads from:

  ```bash
  simard memory stats        # counts should increase after the thread runs
  simard memory dump | less  # inspect the newly remembered concepts/procedures
  ```

  These are written by the recipe's [`simard memory remember` /
  `remember-procedure`](../reference/simard-memory-remember-cli.md) calls, through
  the daemon's authoritative write-boundary gate.

- **Salience ranking** — the `salience` thread's recipe writes **two** things:
  the numeric ranking file, and durable `salience:<goal_id>` rationale facts.
  Inspect both:

  ```bash
  cat "$SIMARD_STATE_ROOT/state/salience_signal.json"   # numeric ranking
  simard memory dump --type=facts | grep '^salience:'   # free-text rationale facts
  ```

  See the [`simard cognition salience-signal` reference](../reference/simard-cognition-salience-signal-cli.md)
  for the format and the fail-closed read semantics.

- **Goals** — a reflective recipe may propose a goal with
  `simard goal add <priority> "<desc>"`; confirm with your usual goal-board
  inspection.

- **Thread health** — a thread records `ran`/`health`/`consecutive_errors` from
  the recipe's **exit status alone**. A recipe failure is logged **loudly** as a
  health error with the captured stderr tail; it is never recorded as "ran, wrote
  nothing." Watch the daemon log for the thread's health line.

## Confirm a dormant deployment is zero-behavior-change

With both gates OFF (the default), no reflective recipe should ever fire:

```bash
unset SIMARD_COGNITIVE_THREADS_ENABLED
# start the daemon, exercise OODA, then confirm:
grep -c 'reflect-postmortem\|metacognition-appraise\|salience-appraise' daemon.log  # expect 0
```

`state/salience_signal.json` will not be created by a dormant deployment, and the
OODA "Decide" reorder reads it fail-closed — so absence simply means "no salience
advice," and OODA proceeds on its own ordering. This is the guarantee that
shipping the threads dormant changes nothing.

## Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Thread never runs | Master gate OFF, or per-thread gate OFF | Set **both** `SIMARD_COGNITIVE_THREADS_ENABLED` and `SIMARD_THREAD_<NAME>_ENABLED` truthy, restart daemon. |
| Thread runs but no facts appear | Recipe reached the memory tool but the daemon rejected the write | Check `simard memory stats` and the daemon log; the write-boundary gate may have quarantined/deduped the fact. |
| Salience file never appears | `salience` thread disabled, or its recipe exited non-zero | Confirm the gate; check the thread's health line and stderr tail in the daemon log. |
| Salience present but OODA ignores it | Signal stale / oversized / malformed | The reader is fail-closed by design; re-run the `salience` thread to refresh `generated_epoch`. |

## See also

- [Reflective cognitive threads act via tools](../architecture/reflective-cognitive-threads.md)
- [`simard cognition salience-signal` CLI](../reference/simard-cognition-salience-signal-cli.md)
- [`simard memory remember` CLI](../reference/simard-memory-remember-cli.md)
- [Configure cognitive-thread scheduling](./configure-cognitive-thread-scheduling.md)
- [Add a new cognitive thread](./add-a-new-cognitive-thread.md)
