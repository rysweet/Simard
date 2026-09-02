---
title: Configure the reflective cognitive threads
description: >
  How to enable, disable, and observe the nine reflective cognitive threads (reflection,
  metacognition, salience, narrative, consolidation, prospection, values-deliberation,
  analogy, operator-model). Covers the default-ON opt-out gate model, the per-thread
  environment switches, verifying that a thread's effects land through the memory tools,
  and how to opt a thread (or the whole roster) out.
last_updated: 2026-07-27
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

## The default-ON opt-out gate model

As of issue #4845 every reflective thread is **ENABLED by default** and runs
unless you **opt it out** through one of two gates:

1. **Master gate** — `SIMARD_COGNITIVE_THREADS_ENABLED` controls whether the
   `Mind` hosts the reflective roster at all. Default **ON**; set it to a falsy
   token to disable the whole roster.
2. **Per-thread gate** — `SIMARD_THREAD_<NAME>_ENABLED` controls one specific
   thread. Default **ON**; set it to a falsy token to disable just that thread.

A gate is an **opt-out**: unset, empty, or any non-falsy value leaves the thread
ENABLED. A thread is disabled only when its master gate **or** its per-thread
gate is set to an explicit falsy token — `0`, `false`, `no`, or `off`
(case-insensitive, surrounding whitespace ignored). This is the flip introduced
by issue #4845; see
[Cognitive-thread full activation](../reference/cognitive-thread-full-activation.md).

To retune (rather than disable) a thread, override its cadence with
`SIMARD_THREAD_<NAME>_INTERVAL_SECS` (seconds, clamped up to
`MIN_INTERVAL_SECS = 60`).

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

## Run the threads (the default)

On a stock daemon **all** reflective threads are already enabled — just start
the daemon:

```bash
simard daemon
```

Each thread ticks on its own cadence (see
[Cognitive-thread full activation](../reference/cognitive-thread-full-activation.md)
for the roster). The startup lines in `ooda.log` name every registered thread as
`ENABLED (interval=<secs>s)` or `DISABLED (operator opt-out)`, so you can confirm
exactly what is running.

## Opt a single thread out

Set that thread's per-thread gate to a falsy token, then (re)start the daemon so
it picks up the environment:

```bash
export SIMARD_THREAD_REFLECTION_ENABLED=0
simard daemon
```

Only `reflection` is disabled; the other threads keep running on their defaults.

## Override a thread's cadence

To keep a thread on but change how often it fires, set its interval override
(seconds, clamped up to `MIN_INTERVAL_SECS = 60`):

```bash
export SIMARD_THREAD_SALIENCE_INTERVAL_SECS=300   # run salience every 5 minutes
simard daemon
```

## Disable the whole reflective roster

Set the **master** gate to a falsy token; the per-thread gates then no longer
matter — nothing in the reflective roster runs:

```bash
export SIMARD_COGNITIVE_THREADS_ENABLED=0   # opts the whole roster out
simard daemon
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

## Confirm an opted-out deployment runs nothing

With the master gate opted out, no reflective recipe should ever fire:

```bash
export SIMARD_COGNITIVE_THREADS_ENABLED=0
# start the daemon, exercise OODA, then confirm:
grep -c 'reflect-postmortem\|metacognition-appraise\|salience-appraise' daemon.log  # expect 0
```

`state/salience_signal.json` will not be created while `salience` is opted out,
and the OODA "Decide" reorder reads it fail-closed — so absence simply means "no
salience advice," and OODA proceeds on its own ordering. This is what an operator
opt-out guarantees.

## Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Thread never runs | Master gate or per-thread gate opted out (set to `0`/`false`/`no`/`off`) | Clear the falsy `SIMARD_COGNITIVE_THREADS_ENABLED` / `SIMARD_THREAD_<NAME>_ENABLED` (unset or set truthy), restart daemon. |
| Thread runs but no facts appear | Recipe reached the memory tool but the daemon rejected the write | Check `simard memory stats` and the daemon log; the write-boundary gate may have quarantined/deduped the fact. |
| Salience file never appears | `salience` thread disabled, or its recipe exited non-zero | Confirm the gate; check the thread's health line and stderr tail in the daemon log. |
| Salience present but OODA ignores it | Signal stale / oversized / malformed | The reader is fail-closed by design; re-run the `salience` thread to refresh `generated_epoch`. |

## See also

- [Reflective cognitive threads act via tools](../architecture/reflective-cognitive-threads.md)
- [`simard cognition salience-signal` CLI](../reference/simard-cognition-salience-signal-cli.md)
- [`simard memory remember` CLI](../reference/simard-memory-remember-cli.md)
- [Configure cognitive-thread scheduling](./configure-cognitive-thread-scheduling.md)
- [Add a new cognitive thread](./add-a-new-cognitive-thread.md)
