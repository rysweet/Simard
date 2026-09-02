---
title: Read a cognitive thread's natural-language reasoning
description: >
  How to see the natural-language reasoning_summary each of Simard's thirteen cognitive
  threads now emits — in the daemon log, and directly from the typed ThreadReasoningRecord
  on disk — and how to enable, disable, and tune the threads. Replaces the old boolean
  "<recipe>: ok" log line with real per-thread reasoning.
last_updated: 2026-07-28
owner: simard
doc_type: howto
status: implemented
related:
  - ../reference/simard-cognition-record-thread-reasoning-cli.md
  - ../architecture/metacognitive-model.md
  - ../reference/cognitive-threads-catalog.md
  - ../howto/configure-cognitive-thread-batch.md
  - ../reference/cognitive-thread-observability.md
---

# Read a cognitive thread's natural-language reasoning

Each of Simard's thirteen cognitive threads records a natural-language
`reasoning_summary` through a typed
[`ThreadReasoningRecord`](../reference/simard-cognition-record-thread-reasoning-cli.md).
This guide shows how to read that reasoning in the daemon log and from disk, and how
to enable/disable/tune the threads. For the whole architecture, see
[The metacognitive model](../architecture/metacognitive-model.md).

## Read it in the daemon log

The daemon logs each thread's outcome summary. With the threads running, you will
see the thread's **actual reasoning** — not the old `"<recipe>: ok"`:

```console
$ simard daemon logs --follow | grep '^cognitive-thread:'
cognitive-thread: salience: prioritising goal #4970 over #4812 because a release-blocking regression outranks docs polish
cognitive-thread: interoception: disk at 91% breaches the 85% guard; filed a capacity health-goal
cognitive-thread: maintenance: reclaimed 2.1 GiB from 3 stale worktrees; skipped 1 under the safety gate
cognitive-thread: reflection: engineer #4801 stalled on flaky CI, not code; recommending a retry on the coverage-comment step
```

Only `reasoning_summary` reaches the log line. A thread whose recipe ran but wrote
no valid record is logged as a **failure**, not a silent `ok`:

```console
cognitive-thread: prospection: FAILED — R5 reasoning_summary invalid (empty/too-short/too-long after sanitize)
```

## Read the record on disk

Each thread writes one owner-only (`0o600`) JSON record, pre-truncated every
invocation, under the state root:

```console
$ ls -l ~/.simard/cognitive_threads/reasoning/
-rw------- 1 simard simard  312 Jul 28 19:40 salience.json
-rw------- 1 simard simard  268 Jul 28 19:41 interoception.json

$ jq . ~/.simard/cognitive_threads/reasoning/salience.json
{
  "schema": "thread-reasoning/v1",
  "thread": "salience",
  "reasoning_summary": "prioritising goal #4970 over #4812 because a release-blocking regression outranks docs polish",
  "written_at_epoch": 1793558400,
  "domain": {
    "kind": "salience",
    "top_signals": ["regression:#4970", "docs:#4812"],
    "priority": 0.92
  }
}
```

!!! note "The record is ephemeral"
    There is no history or rotation — one file per thread, overwritten each
    invocation, with a freshness TTL of `MAX_AGE_SECS = 300` seconds. To keep a
    trail of reasoning over time, read the **daemon log**, which persists each
    summary as it is produced. The on-disk record is the live handoff, not an
    archive.

## What the domain fields mean

Beyond `reasoning_summary`, each record carries a small closed set of per-thread
domain fields for consumers, tests, and audit. See the
[per-domain field table](../reference/simard-cognition-record-thread-reasoning-cli.md#per-domain-fields-closed-set)
for the full list. For example, `salience` carries `top_signals` + `priority`,
`maintenance` carries `candidates` + `freed_bytes`, and the eight reflective
threads without a specialized domain carry a `notes` list.

## Enable, disable, and tune the threads

The threads are **ENABLED by default (opt-out)** behind a default-ON double env
gate. A thread runs unless a gate is set to an explicit falsy token
(`0`/`false`/`no`/`off`).

```bash
# Disable ALL cognitive threads (master gate):
export SIMARD_COGNITIVE_THREADS_ENABLED=0

# Disable just one thread (per-thread gate):
export SIMARD_THREAD_SALIENCE_ENABLED=0

# Re-enable (default is on; unset or set to a truthy token):
unset SIMARD_THREAD_SALIENCE_ENABLED
```

Cadence and priority per thread come from the
[cognitive-threads catalog](../reference/cognitive-threads-catalog.md); to change
the batch configuration see
[Configure the cognitive-thread batch](../howto/configure-cognitive-thread-batch.md).

## Troubleshooting

The daemon logs every failed read with the canonical format
`cognitive-thread: <thread>: FAILED — R{n} <reason>`; the per-check tails are pinned
in the [reference](../reference/simard-cognition-record-thread-reasoning-cli.md#canonical-failure-log-format).

| Symptom | Likely cause | Fix |
|---|---|---|
| `FAILED — R5 reasoning_summary invalid` | The recipe ran but did not call `record-thread-reasoning`, or its summary failed R5 (empty / <8 graphemes / >600 bytes / control-only). | Check the recipe's ACT step; ensure `--reasoning-summary` is a real 1–3 sentence summary. |
| `FAILED — R6 identity mismatch` | The record's `thread` field does not match the invoking thread. | The recipe passed the wrong `--thread`; it must equal the thread the rail invoked. |
| `FAILED — R7 freshness/anti-replay` | The record is stale (`mtime < invoke_start`, older than `MAX_AGE_SECS`, or `written_at_epoch` skewed). | The recipe took >5 min, or the clock is skewed; re-run and verify system time. |
| `FAILED — R4 closed-type parse` | Unknown `--domain` tag, a domain that does not match `--thread`, or an unknown extra field. | Use the domain tag paired with the thread in the [field table](../reference/simard-cognition-record-thread-reasoning-cli.md#per-domain-fields-closed-set). |
| `FAILED — R1 no record at expected path` | The recipe never wrote a record at `state_root/cognitive_threads/reasoning/<thread>.json`. | Confirm the recipe's ACT step passes the rail-supplied `--record-path` unchanged. |
| No `cognitive-thread:` lines at all | Master or per-thread gate set to a falsy token. | Unset the gate or set it to a truthy value. |

## See also

- [`simard cognition record-thread-reasoning` reference](../reference/simard-cognition-record-thread-reasoning-cli.md) —
  the record schema, R1–R7 read matrix, and CLI contract.
- [The metacognitive model](../architecture/metacognitive-model.md) — the whole
  cognitive architecture.
- [Cognitive-thread observability](../reference/cognitive-thread-observability.md) —
  per-thread telemetry and health.
